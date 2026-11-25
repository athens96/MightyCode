// The sidebar (spec section 2; MainWindow.Sidebar.cs, Navigation.cs, Delegation.cs): search, the work-status entry, the
// workspaces with their panes and delegated children, the layout choice and the footer. Every row's words, counts and state
// are the host's (WebShellModel); every click runs a window action.
import { memo, useCallback, useEffect, useState } from 'react';
import { shallowEqual, useServices, useStoreSelector } from '../app/context';
import { ProviderMark } from '../conversation/ProviderMark';
import { activeSessionId } from '../store/store';
import { chooseKeyboard } from '../panes/terminal/focus';
import { useAddMenu, useWorkspaceAddChoices } from './addMenu';
import { sidebarGroups } from './servers';
import { nestForks } from './forks';
import { nextRunWords } from './botTime';
import { atWork, botCounts, workspaceItems, botMatches, botState, groupByState, listedBots, saveAllPanes, savedAllPanes, savedTab, saveTab, sidebarTabEvent, statusLine, switchSidebarTab, type BotState, type SidebarTab } from './botTab';
import { macHost } from './menu';
import { openBotWizard } from './BotWizard';
import { openReportBox } from './ReportBox';
import { clockOf, statusMarks } from './reportRules';
import { BetaBadge, StatusCounts, StatusGlyph, useCall, useMenu, usePaneInfo, useShell, type MenuItem } from './parts';
import type { CardAction, ChildCard, PaneCompare, PaneInfo, RemoteServer, SidebarBot, SidebarItem, SidebarWorkspace } from './types';
import { Icon } from '../icons/Icon';
import appIcon from '../assets/app-icon.png';
import { Pending } from '../pending/Pending';
import { PaneSignals } from '../pane/PaneSignals';
import { setReviewQueue, toggleReview, useReviewOpen, useReviewQueue, type ReviewQueue } from '../review/queue';

export const sidebarSearchId = 'sidebar-search';

/**
 * 검토: the review queue's entry (review/ReviewPanel.tsx) with what is waiting as its badge. The queue is asked for once as the sidebar
 * mounts (the host reads it the first time); the host's pushes keep the badge up to date.
 */
export function ReviewEntry() {
  const { t, bridge } = useServices();
  const queue = useReviewQueue();
  const open = useReviewOpen();
  useEffect(() => { void bridge.call<ReviewQueue>('reviewQueue').then(setReviewQueue, () => {}); }, [bridge]);
  const count = queue?.count ?? 0;
  return (
    <button type="button" className={'sidebar-entry' + (open ? ' selected' : '')} title={t('review.sidebarHelp')} data-testid="sidebar-review" onClick={toggleReview}>
      <span className="sidebar-entry-tile review-tile" aria-hidden="true"><Icon name="list-checks" size={12} weight={2.6} /></span>
      <span className="sidebar-entry-title">{t('review.sidebar')}</span>
      {count > 0 && <span className="sidebar-review-count" data-testid="sidebar-review-count" aria-label={t('review.summary', { count })}>{count}</span>}
    </button>
  );
}

/** Makes a pane the active one: drawn at once, then the host's SelectLayoutSession saves it. */
export function useChoosePane(): (id: string, method?: 'selectSession' | 'activatePane') => void {
  const { bridge, store } = useServices();
  return useCallback((id: string, method: 'selectSession' | 'activatePane' = 'selectSession') => {
    store.dispatch({ kind: 'choose', paneId: id });
    // A choice the host turned down (a dialog was open) is forgotten after a while; one it took is forgotten as its state arrives.
    void bridge.call(method, { id }).catch(() => {}).finally(() => setTimeout(() => { if (store.getState().chosen === id) store.dispatch({ kind: 'choose', paneId: undefined }); }, 1500));
  }, [bridge, store]);
}

function childMenu(call: (method: string, args?: unknown) => void, id: string, actions: readonly CardAction[] | undefined): MenuItem[] {
  return (actions ?? []).map((a) => ({ label: a.label, action: () => call('childAction', { id, action: a.action }) }));
}

function ChildCardLine({ id, card }: { id: string; card: ChildCard }) {
  const { t } = useServices();
  const call = useCall();
  return (
    <div className="child-card" data-testid={'child-card-' + id}>
      {card.busy ? <div className="child-card-busy"><Pending busy showsLabel={false} />{t('delegation.card.working')}</div> : card.actions.length > 0 && (
        <div className="child-card-actions">
          {card.actions.map((a) => (
            <button key={a.action} type="button" className={'child-card-action' + (a.action === 'discard' ? ' quiet' : '')} title={a.help} aria-label={a.help}
              onClick={() => call('childAction', { id, action: a.action })}>{a.label}</button>
          ))}
        </div>
      )}
      {card.held && <div className="child-card-held" title={t('delegation.card.heldNoticesHelp')}>{card.held}</div>}
      {card.note && <div className={'child-card-note tone-text-' + (card.noteTone ?? 'idle')}>{card.note}</div>}
      {card.kept && <div className="child-card-note tone-text-wait">{card.kept}</div>}
    </div>
  );
}

/** MC-29: the words of a compare run's card: how many children, then how many of them run, reported, failed, merged, were discarded. */
export function compareCardWords(card: PaneCompare, t: (key: string, subs?: Readonly<Record<string, string | number>>) => string): { title: string; counts: string } {
  const counts = [
    card.running > 0 ? t('compare.card.running', { count: card.running }) : null,
    card.reported > 0 ? t('compare.card.reported', { count: card.reported }) : null,
    card.failed > 0 ? t('compare.card.failed', { count: card.failed }) : null,
    card.merged > 0 ? t('compare.card.merged', { count: card.merged }) : null,
    card.discarded > 0 ? t('compare.card.discarded', { count: card.discarded }) : null,
  ].filter((part): part is string => part !== null);
  return { title: t('compare.card.title', { count: card.total }), counts: counts.join(' · ') };
}

/** MC-29: a compare run's card under the pane that started it; pressed, the compare view opens. */
function CompareCards({ paneId, cards }: { paneId: string; cards: readonly PaneCompare[] }) {
  const { t } = useServices();
  return (
    <>
      {cards.map((card) => {
        const words = compareCardWords(card, t);
        return (
          <button key={card.id} type="button" className="compare-card" data-testid={'compare-card-' + card.id} title={card.title + '\n' + t('compare.card.open')}
            onClick={() => window.dispatchEvent(new CustomEvent('mighty:open-compare', { detail: { paneId, compareId: card.id } }))}>
            <Icon name="columns-3" size={11} weight={2.2} />
            <span className="compare-card-title">{words.title}</span>
            <span className="compare-card-counts">{words.counts}</span>
          </button>
        );
      })}
    </>
  );
}

/** A pane's row (SessionIndicator): glyph, title in its weight, beta, meta line, waiting words; a delegated child's state word instead. */
/** The sidebar width (px) under which a row's ports and pull request show as icons alone (MC-26). */
const narrowSidebar = 240;
const sidebarDefaultWidth = 252;

const PaneRow = memo(function PaneRow({ info, active, child }: { info: PaneInfo; active: boolean; child?: Extract<SidebarItem, { type: 'pane' }> }) {
  const { t } = useServices();
  const call = useCall();
  const menu = useMenu();
  const choose = useChoosePane();
  const narrow = useStoreSelector((state) => (state.snapshot?.sidebarWidth ?? sidebarDefaultWidth) < narrowSidebar);
  // MC-28: the source's title as it is now (its saved one when it is closed).
  const forkTitle = useStoreSelector((state) => (info.forkedFrom ? state.shell?.panes.find((p) => p.id === info.forkedFrom?.paneId)?.title : undefined));
  // A mirrored pane carries its host's name; its close closes it on that computer.
  const items: MenuItem[] = [
    ...(info.mirror ? [] : [{ label: t('menu.rename'), action: () => call('renameSession', { id: info.id }) }]),
    // MC-58: a top-level agent pane of this computer may be a bot (bot.rs).
    ...(info.kind === 'claude' && !info.mirror && !child ? [{ label: t('bot.menu.edit'), action: () => call('paneBot', { paneId: info.id }) }] : []),
    { label: t('menu.closePane'), action: () => call('closeSession', { id: info.id }) },
  ];
  if (child?.menu?.length) items.push({ separator: true }, ...childMenu(call, info.id, child.menu));
  return (
    <button type="button" className={'sidebar-pane' + (active ? ' active' : '')} data-session-id={info.id} data-testid="sidebar-pane" aria-current={active ? 'true' : undefined}
      title={(info.help || info.title) + '\n' + t('tooltip.sidebar.paneRowHint')}
      onClick={() => { choose(info.id); chooseKeyboard(info.id); }} onContextMenu={(event) => menu.atPointer(event, items)}>
      <StatusGlyph tone={info.tone} kind={info.kind} />
      <span className="sidebar-pane-text">
        <span className="sidebar-pane-line">
          <span className={'sidebar-pane-title' + (info.settled ? '' : ' strong')}>{info.title}</span>
          {info.beta && <BetaBadge />}
          {/* MC-58: a bot's handle, the name it is called by. */}
          {/* MC-61, MC-68: a click on it shows the bot in the 봇 tab (and opens the pane, as the row does). */}
          {info.bot && <span className="sidebar-bot-mark" title={t('bot.label', { handle: info.bot }) + '\n' + t('bot.sidebar.showInList')} data-testid="sidebar-bot-mark"
            onClick={(event) => { event.stopPropagation(); choose(info.id); showBotInList(info.id); }}>@{info.bot}</span>}
          {info.worktree && <span className="sidebar-worktree-mark" title={info.worktree} aria-label={info.worktree} data-testid="sidebar-worktree-mark"><Icon name="git-branch" size={11} weight={2.2} /></span>}
          {/* MC-28: a forked pane carries the fork mark; its source is named on the meta line. */}
          {info.forkedFrom && <span className="sidebar-fork-mark" title={t('fork.label', { title: forkTitle ?? info.forkedFrom.title })} aria-label={t('fork.label', { title: forkTitle ?? info.forkedFrom.title })} data-testid="sidebar-fork-mark"><Icon name="git-fork" size={11} weight={2.2} /></span>}
          {/* MC-26: its ports and pull request (icons only when the sidebar is narrow), and the dot of a pane waiting for the user. */}
          <PaneSignals info={info} compact={narrow} interactive={false} />
          {info.attention && <span className="sidebar-attention-dot" title={info.attention.help} aria-label={info.attention.help} data-testid="sidebar-attention" />}
        </span>
        <span className="sidebar-pane-meta">
          {info.agent && <ProviderMark provider={info.agent} size={11} />}
          {info.agent && <span>{info.agentLabel}</span>}
          <span className="sidebar-pane-detail">{info.forkedFrom ? '↳ ' + (forkTitle ?? info.forkedFrom.title) + (info.meta ? ' ' + info.meta : '') : info.meta}</span>
        </span>
      </span>
      {child?.stateWord ? <span className={'sidebar-pane-wait tone-text-' + (child.stateTone ?? 'idle')}>{child.stateWord}</span>
        : info.wait && <span className="sidebar-pane-wait tone-text-wait">{info.wait}</span>}
    </button>
  );
});

function WorkspaceGroup({ workspace, selected, activeId, panes, bots, dashboard }: { workspace: SidebarWorkspace; selected: boolean; activeId?: string; panes: Map<string, PaneInfo>; bots: readonly SidebarBot[]; dashboard: boolean }) {
  const { t } = useServices();
  const call = useCall();
  const menu = useMenu();
  const addMenu = useAddMenu();
  const worktreeMode = useStoreSelector((state) => state.snapshot?.workspaces.find((w) => w.id === workspace.id)?.paneWorktreeMode ?? 'ask');
  // A mirror workspace is the host's: named and shared there, it leaves when that computer stops sharing it.
  const items: MenuItem[] = workspace.mirror ? [] : [
    { label: t('menu.rename'), action: () => call('renameWorkspace', { id: workspace.id }) },
    { label: t('workspace.menu.remove'), action: () => call('removeWorkspace', { id: workspace.id }) },
  ];
  if (workspace.local) items.push(
    { label: t('menu.showInExplorer'), action: () => call('showInExplorer', { id: workspace.id }) },
    { separator: true },
    { label: t('menu.showFiles'), action: () => call('openFiles', { workspaceId: workspace.id }) },
    { label: t('workMap.open'), action: () => call('openWorkMap', { workspaceId: workspace.id }) },
    // 대화 검색 is the Rust host's (conversation_search.rs).
    ...(import.meta.env.MODE === 'tauri' ? [
      { label: t('git.workbench.open'), action: () => window.dispatchEvent(new CustomEvent('mighty:open-git-workbench', { detail: { workspaceId: workspace.id, paneId: null } })) },
      { label: t('projectTasks.open'), action: () => window.dispatchEvent(new CustomEvent('mighty:open-tasks', { detail: { workspaceId: workspace.id, paneId: null } })) },
      { label: t('agentContext.open'), action: () => window.dispatchEvent(new CustomEvent('mighty:open-agent-context', { detail: { workspaceId: workspace.id, paneId: null } })) },
      { label: t('sharedMemory.open'), action: () => window.dispatchEvent(new CustomEvent('mighty:open-shared-memory', { detail: { workspaceId: workspace.id, paneId: null } })) },
      { label: t('conversationSearch.open'), action: () => call('openConversationSearch', { workspaceId: workspace.id }) },
      { label: t('integrations.menu.browse'), action: () => window.dispatchEvent(new CustomEvent('mighty:open-issues', { detail: { workspaceId: workspace.id, paneId: null, settingsOpen: false } })) },
      { label: t('integrations.menu.settings'), action: () => window.dispatchEvent(new CustomEvent('mighty:open-issues', { detail: { workspaceId: workspace.id, paneId: null, settingsOpen: true } })) },
      // MC-34: the workspace's sandbox (on the Mac; Windows says it is not supported yet).
      { label: t('sandbox.menu.settings'), action: () => window.dispatchEvent(new CustomEvent('mighty:open-sandbox', { detail: { workspaceId: workspace.id } })) },
    ] : []),
    // Where its agent panes work and what each new worktree gets (MainWindow.PaneWorktree.cs WorkspaceWorktreeAction).
    { separator: true },
    { label: t('paneWorktree.menu.mode'), disabled: true },
    ...(['ask', 'further', 'every'] as const).map((mode) => ({
      label: (worktreeMode === mode ? '✓ ' : '    ') + t('paneWorktree.mode.' + mode),
      action: () => call('workspaceWorktree', { workspaceId: workspace.id, action: 'mode', mode }),
    })),
    { label: t('paneWorktree.menu.setup'), action: () => call('workspaceWorktree', { workspaceId: workspace.id, action: 'setup' }) },
    { label: t('paneWorktree.menu.copy'), action: () => call('workspaceWorktree', { workspaceId: workspace.id, action: 'copy' }) },
    { label: t('paneWorktree.menu.list'), action: () => call('workspaceWorktree', { workspaceId: workspace.id, action: 'list' }) },
  );
  else if (workspace.ssh) items.push(
    { separator: true },
    // MC-33: an SSH machine's files, read through ssh (read-only).
    { label: t('menu.showFiles'), action: () => call('openFiles', { workspaceId: workspace.id }) },
    // MC-47: its history by version, read on the machine.
    { label: t('workMap.open'), action: () => call('openWorkMap', { workspaceId: workspace.id }) },
    // MC-47: its agent panes' worktrees, on the machine.
    { separator: true },
    { label: t('paneWorktree.menu.mode'), disabled: true },
    ...(['ask', 'further', 'every'] as const).map((mode) => ({
      label: (worktreeMode === mode ? '✓ ' : '    ') + t('paneWorktree.mode.' + mode),
      action: () => call('workspaceWorktree', { workspaceId: workspace.id, action: 'mode', mode }),
    })),
    { label: t('paneWorktree.menu.setup'), action: () => call('workspaceWorktree', { workspaceId: workspace.id, action: 'setup' }) },
    { label: t('paneWorktree.menu.copy'), action: () => call('workspaceWorktree', { workspaceId: workspace.id, action: 'copy' }) },
    { label: t('paneWorktree.menu.list'), action: () => call('workspaceWorktree', { workspaceId: workspace.id, action: 'list' }) },
  );
  else if (import.meta.env.MODE === 'tauri') items.push(
    ...(items.length > 0 ? [{ separator: true }] : []),
    { label: t('sharedMemory.open'), action: () => window.dispatchEvent(new CustomEvent('mighty:open-shared-memory', { detail: { workspaceId: workspace.id, paneId: null } })) },
  );
  const disclosure = t(workspace.expanded ? 'workspace.collapseAccessibility' : 'workspace.expandAccessibility', { workspace: workspace.name });
  return (
    <section className={'sidebar-workspace' + (selected ? ' selected' : '')} data-workspace-id={workspace.id} data-testid="sidebar-workspace">
      <div className="sidebar-workspace-row" onContextMenu={(event) => items.length > 0 && menu.atPointer(event, items)}>
        <button type="button" className="sidebar-workspace-select" title={t('tooltip.sidebar.workspaceRow', { path: workspace.path })} onClick={() => call('selectWorkspace', { id: workspace.id })}>
          <Icon name={workspace.local ? 'folder' : 'monitor'} size={15} weight={1.5} className="sidebar-workspace-icon" />
          <span className="sidebar-workspace-names">
            <span className="sidebar-workspace-name">{workspace.name}</span>
            {workspace.remoteHost && <span className="sidebar-workspace-host">{workspace.remoteHost}</span>}
          </span>
          <StatusCounts counts={workspace.counts} />
        </button>
        <button type="button" className="sidebar-disclosure" aria-label={disclosure} title={disclosure} aria-expanded={workspace.expanded}
          onClick={() => call('toggleDisclosure', { id: workspace.id })}>
          <Icon name={workspace.expanded ? 'chevron-down' : 'chevron-right'} size={11} weight={2.6} />
        </button>
      </div>
      {workspace.expanded && (
        <div className="sidebar-workspace-panes">
          {/* MC-75: its bots at work are the 봇 tab's. */}
          {nestForks(workspaceItems(workspace.items ?? [], bots), panes).map(({ item, indent }) => {
            if (item.type === 'parentClosed') return (
              <div key={'closed-' + item.id} className="sidebar-child" style={{ paddingLeft: 22 + item.depth * 16 }}>
                <div className="sidebar-parent-closed" title={item.help} aria-label={item.name} data-testid={'sidebar-parent-closed-' + item.id}>
                  <Icon name="monitor-x" size={12} weight={1.75} className="sidebar-parent-closed-icon" />
                  <span className="sidebar-parent-closed-label">{item.label}</span>
                  {item.branch && <span className="mono sidebar-parent-closed-branch">{item.branch}</span>}
                  <span className="spacer" />
                  <span className="mono sidebar-parent-closed-count">{item.count}</span>
                </div>
              </div>
            );
            if (item.type === 'closedChild') return (
              <div key={item.id} className="sidebar-child" style={{ paddingLeft: 22 + item.depth * 16 }}>
                <div className="sidebar-closed-child" title={item.title + ' — ' + t('tooltip.sidebar.closedChild')} data-testid={'sidebar-child-' + item.id} onContextMenu={(event) => item.menu.length > 0 && menu.atPointer(event, childMenu(call, item.id, item.menu))}>
                  <StatusGlyph tone={item.tone} />
                  <span className="sidebar-pane-title">{item.title}</span>
                  <span className={'sidebar-pane-wait tone-text-' + item.stateTone}>{item.stateWord}</span>
                </div>
                {item.card && <ChildCardLine id={item.id} card={item.card} />}
              </div>
            );
            const info = panes.get(item.id);
            if (!info) return null;
            return (
              <div key={item.id} className="sidebar-child" style={{ paddingLeft: 22 + (item.depth + indent) * 16 }}>
                <PaneRow info={info} active={!dashboard && item.id === activeId} child={item.depth > 0 ? item : undefined} />
                {item.card && <ChildCardLine id={item.id} card={item.card} />}
                {info.compares && info.compares.length > 0 && <CompareCards paneId={item.id} cards={info.compares} />}
              </div>
            );
          })}
          <button type="button" className="sidebar-add" title={t('workspace.addPaneHelp')} aria-label={t('workspace.addPaneAccessibility', { workspace: workspace.name })}
            data-testid="sidebar-add" onClick={(event) => addMenu(event.currentTarget, { workspaceId: workspace.id })}>
            <Icon name="plus" size={11} weight={2.6} />{t('workspace.addPane')}
          </button>
        </div>
      )}
    </section>
  );
}

/** A server's header over its workspaces (M/RemoteMirrorViews.swift RemoteServerHeader): "원격 · 이름", "주소 · 상태", its dot, connect button and menu. */
function ServerHeader({ server }: { server: RemoteServer }) {
  const { t } = useServices();
  const call = useCall();
  const menu = useMenu();
  const items: MenuItem[] = [];
  // MC-33: an SSH machine: its connection test and the SSH settings (where a folder of it is opened).
  if (server.ssh) {
    if (server.known) items.push({ label: t('ssh.menu.test'), icon: <Icon name="refresh-cw" size={12} />, action: () => call('sshMachineTest', { id: server.id }) });
    items.push({ label: t('ssh.menu.settings'), icon: <Icon name="settings" size={12} />, action: () => call('sshSettings') });
  } else if (server.known && server.connected) {
    items.push({ label: t('remote.mirror.disconnect'), icon: <Icon name="circle-x" size={12} />, action: () => call('serverDisconnect', { id: server.id }) });
  }
  if (!server.ssh) items.push({ label: t('remote.mirror.settings'), icon: <Icon name="settings" size={12} />, action: () => call('connectRemote') });
  return (
    <div className="sidebar-server" title={server.help} data-testid={'sidebar-remote-server-' + server.id} aria-label={server.title + (server.state ? ', ' + server.state : '')}>
      <span className={'sidebar-server-dot tone-bg-' + server.tone} aria-hidden="true" />
      <span className="sidebar-server-names">
        <span className="sidebar-server-title">{server.title}</span>
        {server.line && <span className="sidebar-server-line mono">{server.line}</span>}
      </span>
      <span className="sidebar-server-actions">
        {server.known && !server.connected && !server.ssh && (
          <button type="button" className="sidebar-server-connect" title={t('tooltip.sidebar.serverConnect', { server: server.title })}
            aria-label={`${server.title} ${t('remote.mirror.connect')}`} data-testid={'sidebar-remote-server-connect-' + server.id}
            onClick={() => call('serverConnect', { id: server.id })}>
            <Icon name="network" size={11} weight={2} />
            <span>{t('remote.mirror.connect')}</span>
          </button>
        )}
        <button type="button" className="icon-button sidebar-server-menu" title={t('tooltip.sidebar.serverMenu', { server: server.title })} aria-label={server.title} data-testid={'sidebar-remote-server-menu-' + server.id}
          onClick={(event) => menu.under(event.currentTarget, items)}><Icon name="ellipsis" size={13} weight={2.6} /></button>
      </span>
    </div>
  );
}

/** MC-61: the event a workspace pane row's `@name` sends so the 봇 tab shows that bot (its id the detail). */
const showBotEvent = 'mighty:show-bot';
function showBotInList(id: string) {
  window.dispatchEvent(new CustomEvent<string>(showBotEvent, { detail: id }));
}

/** MC-59: the bot groups folded, kept in this window's storage (every one open unless folded). */
const botCategoriesClosedKey = 'mighty.sidebar.botCategoriesClosed';
/** MC-68: how the 봇 tab groups its bots: by state (the default) or by kind (워크스페이스 봇, 기타 봇). */
const botGroupingKey = 'mighty.sidebar.botGrouping';

/** MC-68: the title of each state group of the 봇 tab. */
const stateTitles: Record<BotState, string> = {
  needsYou: 'bot.sidebar.state.needsYou',
  working: 'bot.sidebar.state.working',
  idle: 'bot.sidebar.state.idle',
  asleep: 'bot.sidebar.state.asleep',
};

function stored<T extends string>(key: string, fallback: T, values: readonly T[]): T {
  try { const value = window.localStorage.getItem(key); return values.includes(value as T) ? value as T : fallback; } catch { return fallback; }
}

/**
 * MC-68: the 봇 tab: the bots at work (every bot with 모든 창 보기), grouped by state (needs you, working, idle, asleep) or by kind,
 * each row its name, where it works and one line saying what it is doing; a click selects its pane, a double click its workspace, the
 * row's menu opens its settings or puts it to sleep. `focus` is a bot a pane row's `@name` asked for: shown (every pane shown if it
 * was left out), brought into view and flashed.
 */
function BotTab({ bots, panes, activeId, search, workspaces, focus, onFocused }: {
  bots: SidebarBot[]; panes: Map<string, PaneInfo>; activeId: string | undefined; search: string; workspaces: SidebarWorkspace[];
  focus: string | undefined; onFocused: () => void;
}) {
  const { t } = useServices();
  const call = useCall();
  const menu = useMenu();
  const choose = useChoosePane();
  const reportsUnread = useStoreSelector((state) => state.shell?.botReportsUnread ?? 0);
  const [allPanes, setAllPanesState] = useState(savedAllPanes);
  const setAllPanes = (all: boolean) => { setAllPanesState(all); saveAllPanes(all); };
  const [grouping, setGroupingState] = useState<'state' | 'kind'>(() => stored(botGroupingKey, 'state', ['state', 'kind'] as const));
  const setGrouping = (next: 'state' | 'kind') => {
    setGroupingState(next);
    try { window.localStorage.setItem(botGroupingKey, next); } catch { /* storage off: this session only */ }
  };
  const [closed, setClosed] = useState<string[]>(() => {
    try { return JSON.parse(window.localStorage.getItem(botCategoriesClosedKey) ?? '[]') as string[]; } catch { return []; }
  });
  const toggleGroup = (key: string) => {
    const next = closed.includes(key) ? closed.filter((k) => k !== key) : [...closed, key];
    setClosed(next);
    try { window.localStorage.setItem(botCategoriesClosedKey, JSON.stringify(next)); } catch { /* storage off: this session only */ }
  };
  const showWorkspace = (id: string | undefined) => {
    if (!id) return;
    switchSidebarTab('workspaces');
    call('selectWorkspace', { id });
    if (workspaces.some((w) => w.id === id && !w.expanded)) call('toggleDisclosure', { id });
    // Once the list has drawn the workspace's open group, its row is brought into view.
    window.requestAnimationFrame(() => document.querySelector(`[data-workspace-id="${CSS.escape(id)}"]`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }));
  };
  const { listed, hidden } = listedBots(bots, panes, allPanes);
  const shown = listed.filter((bot) => botMatches(bot, panes.get(bot.id), search));
  const groups: { key: string; label: string; bots: SidebarBot[] }[] = grouping === 'state'
    ? groupByState(shown, panes).map((group) => ({ key: 'state:' + group.state, label: stateTitles[group.state], bots: group.bots }))
    : [
      { key: 'workspace', label: 'bot.sidebar.workspaceBots', bots: shown.filter((bot) => bot.workspaceId) },
      { key: 'other', label: 'bot.sidebar.otherBots', bots: shown.filter((bot) => !bot.workspaceId) },
    ].filter((group) => group.bots.length > 0);
  // MC-61: a pane row's `@name` asks for its bot: shown, its group opened, the row brought into view and flashed.
  const [flashed, setFlashed] = useState<string | undefined>();
  useEffect(() => {
    if (!focus) return;
    const bot = bots.find((b) => b.id === focus);
    onFocused();
    if (!bot) return;
    if (!listed.some((b) => b.id === focus)) setAllPanes(true);
    const state = botState(bot, panes.get(bot.id));
    const key = grouping === 'state' ? 'state:' + state : bot.workspaceId ? 'workspace' : 'other';
    if (closed.includes(key)) toggleGroup(key);
    setFlashed(focus);
    window.requestAnimationFrame(() => document.querySelector(`[data-testid="sidebar-bot"][data-session-id="${CSS.escape(focus)}"]`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }));
  });
  useEffect(() => {
    if (!flashed) return;
    const timer = window.setTimeout(() => setFlashed(undefined), 1600);
    return () => window.clearTimeout(timer);
  }, [flashed]);
  const row = (bot: SidebarBot) => {
    const info = panes.get(bot.id);
    const items: MenuItem[] = [
      { label: t('bot.menu.edit'), action: () => call('paneBot', { paneId: bot.id }) },
      // MC-64: 재우기 / 깨우기.
      { label: t(bot.asleep ? 'bot.menu.wake' : 'bot.menu.sleep'), action: () => call('botSleep', { paneId: bot.id, asleep: !bot.asleep }) },
      ...(bot.workspaceId ? [{ label: t('bot.menu.showWorkspace'), action: () => showWorkspace(bot.workspaceId) }] : []),
      // MC-73: its reports in the report box.
      { label: t('bot.report.menu'), action: () => openReportBox(bot.handle) },
      { label: t('menu.closePane'), action: () => call('closeSession', { id: bot.id }) },
    ];
    const next = nextRunWords(bot.nextRun);
    const state = botState(bot, info);
    // MC-73: a bot at rest shows its last report instead (its mark, its time, its summary).
    const report = bot.report && (state === 'idle' || state === 'asleep') ? bot.report : undefined;
    const line = report ? `${statusMarks[report.status]} ${clockOf(report.at)} ${report.summary}` : statusLine(bot, info, next, t);
    const jobsLine = bot.jobs ? '\n' + t('bot.sidebar.jobs', { count: String(bot.jobs) }) + (next ? ' · ' + t('bot.sidebar.nextRun', { at: next }) : '') : '';
    return (
      <button key={bot.id} type="button" className={'sidebar-pane sidebar-bot state-' + state + (bot.id === activeId ? ' active' : '') + (bot.id === flashed ? ' flashed' : '') + (bot.asleep ? ' asleep' : '')} data-testid="sidebar-bot" data-session-id={bot.id}
        title={t('bot.label', { handle: bot.handle }) + '\n' + line + jobsLine + (bot.folder ? '\n' + bot.folder : '') + (bot.workspaceId ? '\n' + t('bot.sidebar.doubleClick') : '')}
        onClick={() => choose(bot.id)} onDoubleClick={() => showWorkspace(bot.workspaceId)} onContextMenu={(event) => menu.atPointer(event, items)}>
        <StatusGlyph tone={info?.tone ?? 'idle'} kind={info?.kind ?? 'claude'} />
        <span className="sidebar-pane-text">
          <span className="sidebar-pane-line">
            <span className={'sidebar-pane-title' + (info?.settled === false ? ' strong' : '')}>@{bot.handle}</span>
            {/* MC-61: a workspace bot's workspace after its name. */}
            {bot.workspaceId && bot.workspace && <span className="sidebar-bot-workspace" data-testid="sidebar-bot-workspace">{bot.workspace}</span>}
            {/* MC-64: asleep, or the next scheduled run (when the line says something else). */}
            {bot.asleep && <span className="sidebar-bot-sleep" aria-label={t('bot.sidebar.asleep')} data-testid="sidebar-bot-asleep">💤</span>}
            {!bot.asleep && next && state !== 'idle' && <span className="sidebar-bot-next" data-testid="sidebar-bot-next">⏰ {next}</span>}
            {state === 'needsYou' && <span className="sidebar-attention-dot" title={info?.attention?.help ?? line} aria-label={info?.attention?.help ?? line} />}
            {bot.report?.unread && state !== 'needsYou' && <span className="sidebar-report-unread" title={t('bot.report.unread')} aria-label={t('bot.report.unread')} />}
          </span>
          {/* MC-68: what it is doing, in one line. */}
          <span className="sidebar-pane-meta">
            {info?.agent && <ProviderMark provider={info.agent} size={11} />}
            <span className={'sidebar-pane-detail sidebar-bot-line' + (state === 'needsYou' ? ' tone-text-wait' : '')} data-testid="sidebar-bot-line">{line}</span>
          </span>
        </span>
      </button>
    );
  };
  const options: MenuItem[] = [
    { label: (grouping === 'state' ? '✓ ' : '    ') + t('bot.sidebar.groupByState'), action: () => setGrouping('state') },
    { label: (grouping === 'kind' ? '✓ ' : '    ') + t('bot.sidebar.groupByKind'), action: () => setGrouping('kind') },
    { separator: true },
    { label: (allPanes ? '✓ ' : '    ') + t('bot.sidebar.allPanes'), action: () => setAllPanes(!allPanes) },
  ];
  return (
    <div className="sidebar-bots" role="tabpanel" data-testid="sidebar-bots">
      {/* MC-71: 봇 추가 opens the new-bot wizard, the list's first row. */}
      <button type="button" className="sidebar-list-add" title={t('bot.sidebar.add')} data-testid="sidebar-add-bot" onClick={openBotWizard}>
        <Icon name="plus" size={13} />{t('bot.sidebar.add')}
      </button>
      <div className="sidebar-section sidebar-bots-header">
        <span className="sidebar-section-title">{t(allPanes ? 'bot.sidebar.allPanesTitle' : 'bot.sidebar.atWorkTitle')}</span>
        <span className="sidebar-section-count">{shown.length}</span>
        <span className="spacer" />
        {/* MC-73: the report box, with how many reports wait unread. */}
        <button type="button" className="sidebar-report-button" title={t('bot.report.title')} data-testid="sidebar-reports" onClick={() => openReportBox()}>
          <Icon name="list-checks" size={11} />{t('bot.report.button')}
          {reportsUnread > 0 && <span className="report-box-count">{reportsUnread}</span>}
        </button>
        <button type="button" className="icon-button" title={t('bot.sidebar.options')} aria-label={t('bot.sidebar.options')} data-testid="sidebar-bot-options"
          onClick={(event) => menu.under(event.currentTarget, options)}>
          <Icon name="ellipsis" size={13} weight={2.6} />
        </button>
      </div>
      {groups.map((group) => {
        const folded = closed.includes(group.key);
        return (
          <div key={group.key} className="sidebar-bot-category" data-testid={'sidebar-bot-category-' + group.key}>
            <button type="button" className="sidebar-bot-category-header" aria-expanded={!folded} title={t(folded ? 'bot.sidebar.expand' : 'bot.sidebar.collapse')}
              onClick={() => toggleGroup(group.key)}>
              <Icon name={folded ? 'chevron-right' : 'chevron-down'} size={10} weight={2.6} />
              <span className="sidebar-bot-category-title">{t(group.label)}</span>
              <span className="sidebar-section-count">{group.bots.length}</span>
            </button>
            {!folded && <div className="sidebar-bot-category-rows">{group.bots.map(row)}</div>}
          </div>
        );
      })}
      {shown.length === 0 && search.trim().length === 0 && (
        <div className="sidebar-bots-empty" data-testid="sidebar-bots-empty">
          <span>{t('bot.sidebar.empty')}</span>
          <button type="button" className="sidebar-open-folder" onClick={openBotWizard}><Icon name="plus" size={14} />{t('bot.sidebar.add')}</button>
        </div>
      )}
      {/* MC-68: the panes left out, one click away. */}
      {hidden > 0 && (
        <button type="button" className="sidebar-bots-more" data-testid="sidebar-bots-more" onClick={() => setAllPanes(true)}>
          {t('bot.sidebar.hiddenPanes', { count: hidden })}
        </button>
      )}
      {allPanes && bots.some((bot) => !atWork(bot)) && (
        <button type="button" className="sidebar-bots-more" data-testid="sidebar-bots-fewer" onClick={() => setAllPanes(false)}>
          {t('bot.sidebar.atWorkOnly')}
        </button>
      )}
    </div>
  );
}

/** MC-68: the sidebar's tabs, 워크스페이스 and 봇, with the 봇 tab's count of bots waiting for the user (MC-71: their + is the list's first row). */
function SidebarTabs({ tab, onTab, workspaceCount, botCount, needsYou, working }: {
  tab: SidebarTab; onTab: (tab: SidebarTab) => void; workspaceCount: number; botCount: number; needsYou: number; working: number;
}) {
  const { t } = useServices();
  const mac = import.meta.env.MODE === 'tauri' && macHost();
  const key = (n: number) => (mac ? '⌘' : 'Ctrl+') + n;
  return (
    <div className="sidebar-tabs" data-testid="sidebar-tabs">
      <div className="sidebar-tabs-track" role="tablist" aria-label={t('sidebar.tabs')}>
        <button type="button" role="tab" aria-selected={tab === 'workspaces'} className={tab === 'workspaces' ? 'on' : ''} data-testid="sidebar-tab-workspaces"
          title={t('sidebar.tab.workspacesHelp', { key: key(1) })} onClick={() => onTab('workspaces')}>
          <Icon name="folder" size={13} weight={2} className="sidebar-tab-icon" />
          <span className="sidebar-tab-label">{t('sidebar.workspacesHeader')}</span>
          <span className="sidebar-tab-count" title={t('tooltip.sidebar.workspaceCount', { count: workspaceCount })}>{workspaceCount}</span>
        </button>
        <button type="button" role="tab" aria-selected={tab === 'bots'} className={tab === 'bots' ? 'on' : ''} data-testid="sidebar-tab-bots"
          title={t('sidebar.tab.botsHelp', { key: key(2) }) + (needsYou > 0 ? '\n' + t('sidebar.tab.botsNeedYou', { count: needsYou }) : '') + (working > 0 ? '\n' + t('sidebar.tab.botsWorking', { count: working }) : '')}
          onClick={() => onTab('bots')}>
          <Icon name="bot" size={13} weight={2} className="sidebar-tab-icon" />
          <span className="sidebar-tab-label">{t('bot.sidebar.title')}</span>
          <BetaBadge />
          {needsYou > 0
            ? <span className="sidebar-tab-badge" data-testid="sidebar-tab-bots-badge" aria-label={t('sidebar.tab.botsNeedYou', { count: needsYou })}>{needsYou}</span>
            : <span className={'sidebar-tab-count' + (working > 0 ? ' working' : '')}>{botCount}</span>}
        </button>
      </div>
    </div>
  );
}

export function Sidebar() {
  const { t } = useServices();
  const { hasSnapshot, activeWorkspaceId, theme, activeId } = useStoreSelector(
    (state) => ({
      hasSnapshot: state.snapshot !== null,
      activeWorkspaceId: state.snapshot?.activeWorkspaceId,
      theme: state.snapshot?.theme,
      activeId: activeSessionId(state),
    }),
    shallowEqual,
  );
  const shell = useShell();
  const panes = usePaneInfo();
  const call = useCall();
  const menu = useMenu();
  const addChoices = useWorkspaceAddChoices();
  const [search, setSearch] = useState('');
  // MC-68: the tab shown (kept in this window's storage), and a bot a pane row's `@name` asked the 봇 tab to show.
  const [tab, setTab] = useState<SidebarTab>(savedTab);
  const [focusBot, setFocusBot] = useState<string | undefined>();
  const chooseTab = useCallback((next: SidebarTab) => { setTab(next); saveTab(next); }, []);
  useEffect(() => {
    const onTab = (event: Event) => chooseTab((event as CustomEvent<SidebarTab>).detail);
    const onShow = (event: Event) => { chooseTab('bots'); setFocusBot((event as CustomEvent<string>).detail); };
    window.addEventListener(sidebarTabEvent, onTab);
    window.addEventListener(showBotEvent, onShow);
    return () => { window.removeEventListener(sidebarTabEvent, onTab); window.removeEventListener(showBotEvent, onShow); };
  }, [chooseTab]);
  if (!hasSnapshot || !shell) return <nav className="sidebar" />;
  const dashboard = shell.dashboard !== undefined;
  const groups = sidebarGroups(shell.workspaces, shell.servers, search);
  const listed = groups.local;
  const entryCounts = shell.entry;
  const bots = shell.bots ?? [];
  const counts = botCounts(bots, panes);
  const searching = search.trim().length > 0;
  // MC-68: what the search finds in the tab not shown.
  const workspaceMatches = listed.length + groups.servers.reduce((n, g) => n + g.workspaces.length, 0);
  const botMatchCount = searching ? bots.filter((bot) => botMatches(bot, panes.get(bot.id), search)).length : 0;
  return (
    <nav className="sidebar" aria-label={t('sidebar.workspacesHeader')} data-testid="sidebar">
      <div className="sidebar-search">
        <Icon name="search" size={14} weight={2} />
        <input id={sidebarSearchId} type="search" placeholder={t('sidebar.searchPlaceholder')} aria-label={t('sidebar.searchPlaceholder')} title={t('tooltip.sidebar.search')} value={search}
          onChange={(event) => setSearch(event.target.value)} />
      </div>
      <button type="button" className={'sidebar-entry' + (dashboard ? ' selected' : '')} title={t('dashboard.sidebarHelp')} data-testid="sidebar-dashboard"
        onClick={() => call(dashboard ? 'hideDashboard' : 'showDashboard')}>
        <span className="sidebar-entry-tile" aria-hidden="true"><Icon name="layout-grid" size={12} weight={2.6} fill /></span>
        <span className="sidebar-entry-title">{t('phone.dashboard.title')}</span>
        <StatusCounts counts={entryCounts} />
      </button>
      <ReviewEntry />
      <SidebarTabs tab={tab} onTab={chooseTab} workspaceCount={groups.ownCount} botCount={listedBots(bots, panes, false).listed.length} needsYou={counts.needsYou} working={counts.working} />
      <div className="sidebar-list">
        {/* MC-68: the search's finds in the other tab, one click away. */}
        {tab === 'workspaces' && botMatchCount > 0 && (
          <button type="button" className="sidebar-search-elsewhere" data-testid="sidebar-search-in-bots" onClick={() => chooseTab('bots')}>{t('sidebar.search.inBots', { count: botMatchCount })}</button>
        )}
        {tab === 'bots' && searching && workspaceMatches > 0 && (
          <button type="button" className="sidebar-search-elsewhere" data-testid="sidebar-search-in-workspaces" onClick={() => chooseTab('workspaces')}>{t('sidebar.search.inWorkspaces', { count: workspaceMatches })}</button>
        )}
        {tab === 'workspaces' ? (
          <div className="sidebar-workspaces" role="tabpanel">
            {/* MC-71: the three ways to add a workspace, as the Mac's sidebar menu offers them (M/WorkspaceView.swift:132), the list's first row. */}
            <button type="button" className="sidebar-list-add" title={t('tooltip.sidebar.addWorkspace')} data-testid="sidebar-add-workspace"
              onClick={(event) => menu.under(event.currentTarget, addChoices.map((choice) => ({ label: choice.label, icon: <Icon name={choice.icon} size={12} />, action: choice.run, testId: 'workspace-add-' + choice.key })))}>
              <Icon name="folder-plus" size={13} />{t('workspace.add.menu')}
            </button>
            {listed.length === 0 && groups.servers.every((g) => g.workspaces.length === 0) && (
              <div className="sidebar-empty">{t(searching ? 'sidebar.noSearchResults' : 'dashboard.empty')}</div>
            )}
            {listed.map((workspace) => (
              <WorkspaceGroup key={workspace.id} workspace={workspace} panes={panes} bots={bots} activeId={activeId} dashboard={dashboard}
                selected={!dashboard && activeWorkspaceId === workspace.id} />
            ))}
            {/* Each connected computer's shared workspaces under its own header (the remote mirror). */}
            {groups.servers.map((group) => (
              <div key={group.server.id} className="sidebar-server-group" data-testid="sidebar-server-group">
                <ServerHeader server={group.server} />
                {group.workspaces.map((workspace) => (
                  <WorkspaceGroup key={workspace.id} workspace={workspace} panes={panes} bots={bots} activeId={activeId} dashboard={dashboard}
                    selected={!dashboard && activeWorkspaceId === workspace.id} />
                ))}
              </div>
            ))}
          </div>
        ) : (
          <BotTab bots={bots} panes={panes} activeId={dashboard ? undefined : activeId} search={search} workspaces={shell.workspaces}
            focus={focusBot} onFocused={() => setFocusBot(undefined)} />
        )}
      </div>
      {/* With no workspace listed (none yet, or none matching the search) these are the sidebar's ways in (M/WorkspaceView.swift:164-171). */}
      {tab === 'workspaces' && listed.length === 0 && (
        <div className="sidebar-add-choices">
          {addChoices.map((choice) => (
            <button key={choice.key} type="button" className="sidebar-open-folder" title={choice.tooltip} data-testid={choice.key === 'existing' ? 'sidebar-open-folder' : 'sidebar-empty-add-' + choice.key} onClick={choice.run}>
              <Icon name={choice.icon} size={14} />{choice.label}
            </button>
          ))}
        </div>
      )}
      <footer className="sidebar-footer">
        <img className="sidebar-app-icon" src={appIcon} width={20} height={20} alt="" />
        <span className="sidebar-brand">
          <span className="sidebar-brand-line"><span className="sidebar-brand-name">Mighty Code</span><BetaBadge /></span>
          <span className="sidebar-version mono" title={t('tooltip.sidebar.version', { version: shell.version })}>v{shell.version}</span>
        </span>
        <button type="button" className="icon-button sidebar-footer-button" title={t('tooltip.sidebar.help')} aria-label={t('menu.help')} onClick={() => call('openHelp')}><Icon name="circle-question-mark" size={15} /></button>
        <button type="button" className="icon-button sidebar-footer-button" title={t(theme === 'light' ? 'tooltip.sidebar.themeToDark' : 'tooltip.sidebar.themeToLight')} aria-label={t('sidebar.toggleTheme')} data-testid="theme-toggle"
          onClick={() => call('setTheme', { theme: theme === 'light' ? 'dark' : 'light' })}><Icon name={theme === 'light' ? 'moon' : 'sun'} size={15} /></button>
        <button type="button" className="icon-button sidebar-footer-button" title={t('tooltip.sidebar.settings')} aria-label={t('settings.settingsWindowTitle')} data-testid="sidebar-settings" onClick={() => call('openSettings')}>
          <Icon name="settings" size={15} />
        </button>
      </footer>
    </nav>
  );
}

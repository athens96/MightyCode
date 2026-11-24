import { lazy, memo, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import type { RunSession } from '../store/models';
import { shallowEqual, useServices, useStoreSelector } from '../app/context';
import { Conversation } from '../conversation/Conversation';
import { Composer } from '../composer/Composer';
import { Cards } from '../cards/Cards';
import { clockLabel } from '../composer/clock';
import { MightyView, useShownSession } from '../mighty/MightyView';
import { SlimPane, slimKind } from '../panes/Panes';
import { TerminalView } from '../panes/terminal/TerminalView';
import { ReferenceContext } from '../conversation/referenceClick';
import type { ReferenceTarget } from '../conversation/references';
import type { ReferenceRequest } from './ReferenceBubble';
import { Icon } from '../icons/Icon';
import { providerLabel } from '../conversation/format';
import { drawsDiagram } from '../pending/requests';
import { useCall, useMenu, type MenuItem } from '../shell/parts';
import { WorktreeBar } from './WorktreeBar';
import { MirrorStrip } from './MirrorStrip';
import { SshBar } from './SshBar';
import { ForkStrip, Opinions } from './Opinions';
import { logText } from './log';
import { sandboxed } from '../sandbox/sandboxModel';
import { PreviewButton, PreviewStrip } from '../preview/PreviewStrip';

const LazyReferenceBubble = lazy(() => import('./ReferenceBubble').then((m) => ({ default: m.ReferenceBubble })));

/** The header's clock, redrawn each second while the run is not finished (MW.Composer.cs RefreshElapsed). */
function Clock({ session }: { session: RunSession }) {
  const { t } = useServices();
  const [now, setNow] = useState(() => Date.now());
  const timing = session.runTiming;
  const running = timing !== undefined && !timing.finishedAt;
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);
  if (!timing) return null;
  const elapsed = clockLabel(timing, now, t('dashboard.card.approximate'));
  return <span className="pane-clock" title={t(running ? 'pane.elapsed.runningAccessibility' : 'pane.elapsed.finishedAccessibility', { time: elapsed })} data-testid="pane-clock">{elapsed}</span>;
}

/**
 * The pane's ··· menu (M/SessionPaneView.swift paneMenu): rename, focus or back to the layout, a shell's earlier command history, an
 * agent's log copied, a new conversation, close.
 */
function PaneMenu({ session }: { session: RunSession }) {
  const { bridge, t } = useServices();
  const menu = useMenu();
  const call = useCall();
  const { focused, mirrored } = useStoreSelector(
    (state) => ({
      focused: state.shell?.layout?.mode === 'focus',
      // A mirrored pane carries its host's name: no rename here (M/WorkspaceView.swift).
      mirrored: Boolean(state.shell?.panes.find((p) => p.id === session.id)?.mirror),
    }),
    shallowEqual,
  );
  const items: MenuItem[] = [
    ...(mirrored ? [] : [{ label: t('menu.rename'), action: () => call('renameSession', { id: session.id }) }]),
    { label: t(focused ? 'pane.menu.restoreLayout' : 'menu.focusPane'), action: () => call('focusPane', { id: session.id, toggle: true }) },
  ];
  // A shell's history of what its pane recorded, as the tab's menu has it (Layout.tsx paneMenu, `terminalHistory`).
  if (session.kind === 'shell') items.push({ label: t('pane.menu.terminalHistory'), action: () => call('terminalHistory', { paneId: session.id }) });
  else items.push({ label: t('pane.menu.copyLog'), disabled: session.logs.length === 0, action: () => void navigator.clipboard?.writeText(logText(session.logs, session.provider, t)).catch(() => {}) });
  if (session.kind !== 'shell') items.push({ label: t('pane.menu.newConversation'), disabled: session.status === 'running' || !session.resumeId, action: () => void bridge.call('newConversation', { paneId: session.id }).catch(() => {}) });
  // MC-28: the conversation into a new pane in a new worktree (fork.rs); never a mirror's or while it runs.
  if (session.kind === 'claude' && !mirrored) items.push({ label: t('fork.menu.pane'), disabled: session.status === 'running' || session.logs.length === 0, action: () => call('paneFork', { paneId: session.id }) });
  // MC-58: a top-level agent pane of this computer may be a bot (bot.rs).
  if (session.kind === 'claude' && !mirrored && !session.parentSessionId) items.push({ label: t('bot.menu.edit'), action: () => call('paneBot', { paneId: session.id }) });
  items.push({ separator: true }, { label: t('menu.closePane'), action: () => call('closeSession', { id: session.id }) });
  return (
    <button type="button" className="icon-button pane-menu-button" title={t('tooltip.pane.menu')} aria-label={t('pane.menu.accessibility')} data-testid="pane-menu"
      onClick={(event) => menu.under(event.currentTarget, items)}><Icon name="ellipsis" size={15} weight={2.6} /></button>
  );
}

/**
 * The selected pane: its header (title, state word and figures, spec section 1) over its conversation and, for an agent or a
 * shell, the composer. The word and the figures after the clock are the host's (MainWindow.WebShell.Composer.cs WebHeader).
 */
export const PaneView = memo(function PaneView({ session: saved }: { session: RunSession }) {
  const { t, bridge } = useServices();
  // A view switch clicked a moment ago shows at once (natural submit B4); the same object while none waits for the host.
  const session = useShownSession(saved);
  // Another computer's pane (the remote mirror): its strip under the header; a remote pane's files are on its host: no bubble opens there.
  const { header, mirror, agentTerminal, remote, ssh, boxed } = useStoreSelector(
    (state) => {
      const info = state.shell?.panes.find((p) => p.id === session.id);
      const workspace = state.snapshot?.workspaces.find((w) => w.id === session.workspaceId);
      return {
        header: state.composers[session.id]?.header,
        mirror: info?.mirror,
        agentTerminal: Boolean(info?.agentTerminal),
        // An SSH machine's files are read through the files pane only (MC-33): no reference bubble reads them here.
        remote: Boolean(workspace?.remote || workspace?.ssh),
        ssh: info?.ssh,
        // MC-34: the workspace's agent runs are sandboxed (its badge in the header).
        boxed: sandboxed(workspace),
      };
    },
    shallowEqual,
  );
  const agent = session.kind === 'claude' || session.kind === 'shell';
  // An agent pane with a Mighty view always shows it; the conversation stays for the panes that have none.
  const mighty = drawsDiagram(session);
  const [reference, setReference] = useState<ReferenceRequest | null>(null);
  const clicks = useRef(0);
  const room = useRef<HTMLDivElement>(null);
  const showReference = useCallback((target: ReferenceTarget) => {
    // ⌘/Ctrl+click: the file in the external editor at its line, resolved in the pane's folder by the host (editor.rs).
    if (target.editor) void bridge.call('openInEditor', { source: 'reference', paneId: session.id, path: target.path, ...(target.line ? { line: target.line } : {}) }).catch(() => {});
    else setReference({ ...target, seq: ++clicks.current });
  }, [bridge, session.id]);
  const closeReference = useCallback(() => setReference(null), []);
  if (slimKind(session.kind)) return <SlimPane session={session} />;
  // Under the Mighty bar while the Mighty view shows.
  const bubbleTop = mighty ? room.current?.querySelector<HTMLElement>('.mighty-bar')?.offsetHeight ?? 0 : 0;
  // A shell's terminal stands where the conversation and the composer are; a remote workspace's shell keeps them (its commands go to the host).
  const body = (
    <>
      <ReferenceContext.Provider value={remote ? null : showReference}>
        <div className="pane-room" ref={room}>
          {mighty ? <MightyView key={'mighty-' + session.id} session={session} /> : <Conversation key={session.id} session={session} />}
          {reference && !remote && (
            <Suspense fallback={null}>
              <LazyReferenceBubble paneId={session.id} request={reference} top={bubbleTop} onClose={closeReference} />
            </Suspense>
          )}
        </div>
      </ReferenceContext.Provider>
      {session.kind === 'claude' && session.opinions && session.opinions.length > 0 && <Opinions paneId={session.id} cards={session.opinions} />}
      {session.kind === 'claude' && <Cards key={'cards-' + session.id} paneId={session.id} hideBackground={mighty} />}
      {/* MC-7: the live preview's strip while the pane's preview is open. */}
      {session.kind === 'claude' && !remote && !mirror && <PreviewStrip key={'preview-' + session.id} paneId={session.id} />}
      {agent && <Composer key={'composer-' + session.id} session={session} />}
    </>
  );
  const word = header?.word ?? t('session.state.' + session.status);
  // A shell keeps the slim ink bar (M/PaneChrome.swift SlimPaneHeader): what it is, its state in a pill, its menu.
  if (session.kind === 'shell') return (
    <section className="pane pane-shell" data-session-id={session.id}>
      <header className="pane-slim-header" data-testid="pane-slim-header">
        <h1 className="visually-hidden" data-testid="pane-title">{session.title}</h1>
        <span className="pane-slim-words">{t('dashboard.kind.shell') + (remote ? '' : ' · ' + t('phone.card.localTerminal'))}</span>
        <span className="spacer" />
        <span className="hero-status-pill" title={t('tooltip.pane.status', { status: word })} data-testid="pane-state">{word}</span>
        <PaneMenu session={session} />
      </header>
      <TerminalView key={'terminal-' + session.id} session={session} fallback={body} />
    </section>
  );
  // An agent's header (M/SessionPaneView.swift agentHeader): its state word in its ink, the figures in mono fading at the right,
  // then the pane's menu. The name is on the tab right above, so it is not said twice.
  return (
    <section className="pane" data-session-id={session.id}>
      <header className="pane-header">
        <h1 className="visually-hidden" data-testid="pane-title">{session.title}</h1>
        <span className={'pane-status tone-' + (header?.tone ?? 'idle')} title={t('tooltip.pane.status', { status: word })} data-testid="pane-state">{word}</span>
        <span className="pane-figures" data-testid="pane-figures" title={header?.help}>
          {header?.timed && <Clock session={session} />}
          {header?.rest}
        </span>
        <span className="pane-header-controls">
          {boxed && (
            <button type="button" className="beta-badge sandbox-badge" data-testid="pane-sandbox" title={t('sandbox.badgeHelp')} aria-label={t('sandbox.badgeHelp')}
              onClick={() => window.dispatchEvent(new CustomEvent('mighty:open-sandbox', { detail: { workspaceId: session.workspaceId } }))}>
              <Icon name="shield" size={10} weight={2.4} />{t('sandbox.badge')}</button>
          )}
          {mighty && (session.provider === 'claude' || session.provider === 'codex') && (
            <button type="button" className="icon-button pane-plugins-button" data-testid="pane-plugins" title={t('pane.plugins.help', { provider: providerLabel(session.provider) })}
              aria-label={t('plugins.titleTemplate', { provider: providerLabel(session.provider) })} onClick={() => void bridge.call('openPlugins', { id: session.id }).catch(() => {})}>
              <Icon name="puzzle" size={14} weight={2.2} /></button>
          )}
          {/* MC-7: the workspace's dev servers in a live preview (not an SSH or remote workspace's, nor another computer's pane). */}
          {!remote && !mirror && <PreviewButton paneId={session.id} />}
          {/* The agent's terminal after its pane was closed (M/SessionPaneView.swift `agent-terminal-open-`): the pane back beside it, selected. */}
          {agentTerminal && (
            <button type="button" className="icon-button pane-plugins-button" data-testid="pane-agent-terminal" title={t('agentTerminal.terminalPane.open')}
              aria-label={t('agentTerminal.terminalPane.open')} onClick={() => void bridge.call('terminalPromptShow', { paneId: session.id }).catch(() => {})}>
              <Icon name="terminal" size={14} weight={2.2} /></button>
          )}
          <PaneMenu session={session} />
        </span>
      </header>
      {mirror && <MirrorStrip id={session.id} strip={mirror} />}
      {ssh && <SshBar paneId={session.id} ssh={ssh} />}
      {session.paneWorktree && <WorktreeBar key={'worktree-' + session.id} session={session} />}
      {session.forkedFrom && <ForkStrip link={session.forkedFrom} />}
      {body}
    </section>
  );
});

// The bar over an agent pane in its own worktree (pane/worktree.ts): branch → base, how far it is (↑ ↓ ±), its PORT and port block,
// the ports it listens on and its pull request (MC-26), the run actions running in it (MC-25), its setup, and Merge / Commit and merge, Ask to resolve and the rest in its menu. The host
// does the git (MainWindow.PaneWorktree.cs).
import { useCallback, useEffect, useState } from 'react';
import { useServices, useStoreSelector } from '../app/context';
import { Icon } from '../icons/Icon';
import { useMenu } from '../shell/parts';
import type { LinkedTask, RunSession } from '../store/models';
import { worktreeBar, type WorktreeStatus } from './worktree';
import { PaneSignals } from './PaneSignals';
import './worktree.css';

const TRACKER_SHORT: Record<string, string> = {
  mightywork: 'MightyWork',
  github: 'GitHub',
  linear: 'Linear',
  jira: 'Jira',
  gitlab: 'GitLab',
};

/** MC-31: the linked-issue chip's text: the key, and the tracker's name for Linear, Jira and GitLab (their keys alone look alike). */
export function linkedIssueLabel(linked: LinkedTask): string {
  return linked.source === 'linear' || linked.source === 'jira' || linked.source === 'gitlab'
    ? `${TRACKER_SHORT[linked.source]} ${linked.key}`
    : linked.key;
}

/** The chip's tooltip: tracker · key · title. */
export function linkedIssueTitle(linked: LinkedTask): string {
  const tracker = TRACKER_SHORT[linked.source];
  return [tracker, linked.key, linked.title].filter((part): part is string => Boolean(part)).join(' · ');
}

export function WorktreeBar({ session }: { session: RunSession }) {
  const { t, bridge } = useServices();
  const menu = useMenu();
  const pane = session.paneWorktree!;
  const [status, setStatus] = useState<WorktreeStatus | null>(null);
  const [working, setWorking] = useState(false);
  // MC-26: the ports its processes listen on and its branch's pull request, as the host read them.
  const info = useStoreSelector((state) => state.shell?.panes.find((p) => p.id === session.id));
  const refresh = useCallback(() => {
    void bridge.call<WorktreeStatus | null>('paneWorktreeStatus', { id: session.id }).then((value) => setStatus(value ?? null), () => {});
  }, [bridge, session.id]);
  // Read again when a run ends and every 20 seconds (the Mac's bar).
  useEffect(() => { refresh(); const timer = setInterval(refresh, 20000); return () => clearInterval(timer); }, [refresh, session.status]);
  const act = (action: string) => {
    setWorking(true);
    void bridge.call('paneWorktreeAction', { id: session.id, action }).finally(() => { setWorking(false); refresh(); });
  };
  const model = worktreeBar(pane, status, t);
  const busy = working || !!status?.busy;
  const linked = pane.linkedTask ?? session.linkedTask ?? null;
  return (
    <div className="worktree-bar" data-testid={'pane-worktree-bar-' + session.id}>
      <Icon name="git-branch" size={12} weight={2.2} />
      <span className="worktree-bar-branch" title={model.help}>{model.branch}</span>
      {linked && (
        <button
          type="button"
          className="worktree-bar-port link mono"
          data-testid="pane-worktree-linked-task"
          data-tracker={linked.source}
          title={linkedIssueTitle(linked)}
          onClick={() => { if (linked.url) void bridge.call('openUrl', { url: linked.url }).catch(() => {}); }}
        >
          {linkedIssueLabel(linked)}
        </button>
      )}
      {model.ahead && <span className="worktree-bar-count" title={model.ahead.help}>{model.ahead.text}</span>}
      {model.behind && <span className="worktree-bar-count tone-text-wait" title={model.behind.help}>{model.behind.text}</span>}
      {model.changed && <button type="button" className="worktree-bar-count link" title={model.changed.help} onClick={() => act('changes')}>{model.changed.text}</button>}
      <button type="button" className="worktree-bar-port link" title={model.port.help} onClick={() => act('port')}>{model.port.text}</button>
      {model.portBlock && <span className="worktree-bar-port mono" data-testid="pane-worktree-port-block" title={model.portBlock.help}>{model.portBlock.text}</span>}
      {info && <PaneSignals info={info} compact={false} interactive />}
      {model.actions.map((action) => (
        <button
          key={action.name}
          type="button"
          className="worktree-bar-action link"
          data-testid={'pane-worktree-action-' + action.name}
          title={action.help}
          onClick={() => {
            if (action.url) void bridge.call('openUrl', { url: action.url }).catch(() => {});
            else window.dispatchEvent(new CustomEvent('mighty:open-tasks', { detail: { workspaceId: session.workspaceId, paneId: session.id } }));
          }}
        >
          <Icon name="play" size={9} weight={2.4} />
          <span>{action.name}</span>
        </button>
      ))}
      {model.setup && (
        model.setup.tone === 'err' && import.meta.env.MODE === 'tauri' ? (
          <button
            type="button"
            className={'worktree-bar-setup link tone-text-' + model.setup.tone}
            title={t('projectTasks.askAgentFix')}
            onClick={() => window.dispatchEvent(new CustomEvent('mighty:open-tasks', { detail: { workspaceId: session.workspaceId, paneId: session.id } }))}
          >
            {model.setup.text}
          </button>
        ) : (
          <span className={'worktree-bar-setup tone-text-' + model.setup.tone}>{model.setup.text}</span>
        )
      )}
      <span className="spacer" />
      {model.resolve && <button type="button" className="btn small" title={model.resolve.help} disabled={busy} onClick={() => act('resolve')} data-testid="pane-worktree-resolve">{model.resolve.label}</button>}
      <button type="button" className="btn small primary" title={model.primary.help} disabled={busy} onClick={() => act(model.primary.action)} data-testid="pane-worktree-merge">{model.primary.label}</button>
      <button type="button" className="icon-button" title={t('paneWorktree.bar.more')} aria-label={t('paneWorktree.bar.more')} disabled={busy}
        onClick={(event) => menu.under(event.currentTarget, model.menu.map((item) => ({ label: item.label, action: () => act(item.action) })))}><Icon name="ellipsis" size={14} weight={2.4} /></button>
    </div>
  );
}

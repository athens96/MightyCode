// 검토 대기열 (queue.ts, src-tauri review.rs): every worktree pane, child pane and request that is ready to review in one panel, each
// with its pane, agent, branch, size and state, and the actions that already do the work (their confirmations are the host's
// dialogs, drawn over this panel: it sits on the surface layer). Opened from the sidebar's 검토 entry, ⇧⌘R on the Mac and
// Ctrl+Shift+R on Windows.
import { useCallback, useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon, type IconName } from '../icons/Icon';
import { RunChanges } from '../mighty/RunChanges';
import { usePageOverlay } from '../shell/overlay';
import {
  itemActions, runReviewAction, setReviewOpen, setReviewQueue, useReviewOpen, useReviewQueue,
  type ReviewAction, type ReviewItem, type ReviewKind, type ReviewQueue, type ReviewStatus,
} from './queue';
import './review.css';

/** Each kind's and status's words, named in full so the locale check sees them used. */
const kindKey: Record<ReviewKind, string> = { worktree: 'review.kind.worktree', child: 'review.kind.child', request: 'review.kind.request' };
const statusKey: Record<ReviewStatus, string> = { ready: 'review.status.ready', conflict: 'review.status.conflict', merged: 'review.status.merged' };

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

const actionWords: Record<ReviewAction, string> = {
  changes: 'review.action.changes', merge: 'review.action.merge', commitMerge: 'review.action.commitMerge', discard: 'review.action.discard',
  editor: 'editor.open', pr: 'review.action.pr', dismiss: 'review.action.dismiss', focus: 'review.action.focus',
};

const actionIcons: Partial<Record<ReviewAction, IconName>> = {
  changes: 'file-search', merge: 'git-merge', commitMerge: 'git-merge', editor: 'file-code', pr: 'arrow-up-right', discard: 'trash',
  focus: 'square-arrow-out-up-right', dismiss: 'check',
};

const statusIcons: Record<ReviewItem['status'], IconName> = { ready: 'circle-check', conflict: 'triangle-alert', merged: 'git-merge' };

/** One row: the pane's title over its agent, kind, workspace and branch; its size and state; its actions. */
export function ReviewRow({ item, t, working, onAction }: { item: ReviewItem; t: Translate; working: boolean; onAction: (action: ReviewAction) => void }) {
  const busy = working || item.busy;
  const place = [item.agent, t(kindKey[item.kind]), item.workspace].filter((part) => part !== '').join(' · ');
  return (
    <div className={'review-row status-' + item.status} role="listitem" data-testid={'review-row-' + item.key} data-kind={item.kind} data-status={item.status}>
      <span className={'review-status-icon tone-text-' + (item.status === 'conflict' ? 'err' : item.status === 'merged' ? 'wait' : 'done')} aria-hidden="true">
        <Icon name={statusIcons[item.status]} size={15} weight={1.9} />
      </span>
      <div className="review-words">
        <div className="review-title-line">
          <span className="review-title" title={item.title}>{item.title}</span>
          <span className={'review-status status-' + item.status}>{t(statusKey[item.status])}</span>
        </div>
        <span className="review-place">{place}</span>
        {item.branch && <span className="review-branch mono" title={item.branch + (item.base ? ' → ' + item.base : '')}><Icon name="git-branch" size={11} weight={2.2} />{item.branch}{item.base ? ' → ' + item.base : ''}</span>}
        {item.request && <span className="review-request" title={item.request}>{item.request}</span>}
      </div>
      <span className="review-counts" title={t('review.files', { count: item.files })}>
        <span className="review-files">{t('review.files', { count: item.files })}</span>
        <span className="added">+{item.added}</span> <span className="removed">−{item.removed}</span>
      </span>
      <div className="review-actions">
        {itemActions(item).map((action) => (
          <button key={action} type="button" className={'btn small' + (action === 'discard' ? ' danger' : action === 'merge' || action === 'commitMerge' ? ' primary' : '')}
            data-testid={'review-' + action + '-' + item.key} disabled={busy && action !== 'focus' && action !== 'editor'}
            title={action === 'dismiss' ? t('review.action.dismissHelp') : t(actionWords[action])} onClick={() => onAction(action)}>
            {actionIcons[action] && <Icon name={actionIcons[action]} size={12} weight={2} />}
            <span>{t(actionWords[action])}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

/** The list (or why it is empty, or its wait while the first reading comes). */
export function ReviewList({ queue, t, working, onAction }: { queue: ReviewQueue | null; t: Translate; working: string | null; onAction: (item: ReviewItem, action: ReviewAction) => void }) {
  if (!queue) return <DialogLoading rows={3} />;
  if (queue.items.length === 0) return <div className="review-empty" data-testid="review-empty">{t('review.empty')}</div>;
  return (
    <div className="review-list" role="list" data-testid="review-list">
      {queue.items.map((item) => <ReviewRow key={item.key} item={item} t={t} working={working === item.key} onAction={(action) => onAction(item, action)} />)}
    </div>
  );
}

/** The panel while it is open: read again as it opens, kept up by the host's pushes. */
function ReviewDialog() {
  const { bridge, t } = useServices();
  const dialogs = useStoreSelector((state) => state.dialogs.length);
  const queue = useReviewQueue();
  const [refreshing, setRefreshing] = useState(false);
  const [working, setWorking] = useState<string | null>(null);
  const [runChanges, setRunChanges] = useState<{ paneId: string; runId: string } | null>(null);
  usePageOverlay(true);
  const call = useCallback((method: string, args?: unknown) => bridge.call(method, args), [bridge]);
  const refresh = useCallback(() => {
    setRefreshing(true);
    void bridge.call<ReviewQueue>('reviewQueue', { refresh: true }).then(setReviewQueue, () => {}).finally(() => setRefreshing(false));
  }, [bridge]);
  useEffect(() => { refresh(); }, [refresh]);
  const act = (item: ReviewItem, action: ReviewAction) => {
    setWorking(item.key);
    void runReviewAction(call, item, action, { openRunChanges: (paneId, runId) => setRunChanges({ paneId, runId }) })
      .then((again) => { if (again) refresh(); }, () => {})
      .finally(() => setWorking(null));
  };
  const close = () => setReviewOpen(false);
  return (
    <>
      <DialogShell kind="panel" panel="review" width={860} height={560} minWidth={560} minHeight={300} label={t('review.open')} testId="review-panel"
        layer="surface" top={dialogs === 0 && !runChanges} icon={<Icon name="list-checks" size={18} weight={1.8} />} title={t('review.open')}
        tools={<button type="button" className="icon-button" data-testid="review-refresh" aria-label={t('review.refresh')} title={t('review.refresh')} disabled={refreshing}
          onClick={refresh}><Icon name="refresh-cw" size={14} weight={2} /></button>}
        onCancel={close} closeTestId="review-close" bodyClassName="review-body" backdrop={{ 'data-page-overlay': '' }}
        status={queue ? t('review.summary', { count: queue.count }) : ''}>
        <ReviewList queue={queue} t={t} working={working} onAction={act} />
      </DialogShell>
      {runChanges && <RunChanges paneId={runChanges.paneId} runId={runChanges.runId} onClose={() => { setRunChanges(null); refresh(); }} />}
    </>
  );
}

/** The panel, drawn over the window while it is open. */
export function ReviewPanel() {
  const open = useReviewOpen();
  return open ? createPortal(<ReviewDialog />, document.body) : null;
}

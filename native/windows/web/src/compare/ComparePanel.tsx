// 비교 결과 (MC-29): one column per child of a compare run — agent and model, state, files and +/− lines against the base commit,
// elapsed time, cost, the test's result — with 창 보기, diff 보기 (the changes sheet), 병합 (a reported child; then the host asks
// whether to discard the others) and 폐기. Read as it opens, every 10 s while it is open, and after each action; the host's dialogs
// draw over it (it sits on the surface layer).
import { useCallback, useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { ProviderMark } from '../conversation/ProviderMark';
import { usePageOverlay } from '../shell/overlay';
import { useChoosePane } from '../shell/Sidebar';
import { elapsedWords, phaseKey, phaseTone, testWords, usd, type CompareColumn, type CompareView } from './compare';
import './compare.css';

/** How often the open view reads the children again. */
const REFRESH_MS = 10_000;

function Column({ column, chosenTest, working, act, open }: {
  column: CompareColumn; chosenTest: boolean; working: boolean; act: (action: 'diff' | 'merge' | 'discard') => void; open: () => void;
}) {
  const { t } = useServices();
  const changes = column.files === null ? '—' : column.files === 0 ? t('compare.view.noChanges') : t('compare.view.files', { count: column.files });
  return (
    <div className={'compare-column phase-' + column.phase} role="listitem" data-testid={'compare-column-' + column.id}>
      <div className="compare-column-head">
        <ProviderMark provider={column.agent} size={14} />
        <span className="compare-column-agent">{column.agentLabel}</span>
        <span className="compare-column-model mono" title={column.model ?? t('compare.picker.defaultModel')}>{column.model ?? t('compare.picker.defaultModel')}</span>
      </div>
      <span className={'compare-phase tone-text-' + phaseTone(column.phase)}>{t(phaseKey[column.phase])}</span>
      {column.failure && <div className="compare-note tone-text-err" title={column.failure}>{column.failure}</div>}
      <dl className="compare-facts">
        <dt>{t('compare.view.changes')}</dt>
        <dd>{changes}{column.files ? <> <span className="added">+{column.added ?? 0}</span> <span className="removed">−{column.removed ?? 0}</span></> : null}</dd>
        <dt>{t('compare.view.elapsed')}</dt>
        <dd className="mono">{elapsedWords(column.elapsed)}{column.running && column.elapsed !== null ? ' …' : ''}</dd>
        <dt>{t('compare.view.cost')}</dt>
        <dd className="mono">{column.costUsd === null ? '—' : usd(column.costUsd)}</dd>
        <dt>{t('compare.view.test')}</dt>
        <dd className={column.test?.state === 'failed' ? 'tone-text-err' : column.test?.state === 'passed' ? 'tone-text-done' : ''} title={column.test?.output ?? undefined}>{testWords(column, chosenTest, t)}</dd>
        {column.portBase !== null && column.portCount !== null && <>
          <dt>{t('compare.view.ports')}</dt>
          <dd className="mono">{column.portBase}–{column.portBase + column.portCount - 1}</dd>
        </>}
      </dl>
      <div className="compare-actions">
        <button type="button" className="btn small" disabled={!column.canOpen} title={t('compare.action.open')} onClick={open}>
          <Icon name="square-arrow-out-up-right" size={12} weight={2} /><span>{t('compare.action.open')}</span>
        </button>
        <button type="button" className="btn small" disabled={!column.canDiff || working} title={t('compare.action.diff')} onClick={() => act('diff')}>
          <Icon name="file-search" size={12} weight={2} /><span>{t('compare.action.diff')}</span>
        </button>
        <button type="button" className="btn small primary" data-testid={'compare-merge-' + column.id} disabled={!column.canMerge || working}
          title={column.canMerge ? t('compare.action.merge') : t('compare.action.mergeHelp')} onClick={() => act('merge')}>
          <Icon name="git-merge" size={12} weight={2} /><span>{t('compare.action.merge')}</span>
        </button>
        <button type="button" className="btn small danger" disabled={!column.canDiscard || working} title={t('compare.action.discard')} onClick={() => act('discard')}>
          <Icon name="trash" size={12} weight={2} /><span>{t('compare.action.discard')}</span>
        </button>
      </div>
    </div>
  );
}

export function ComparePanel({ paneId, compareId, onClose }: { paneId: string; compareId: string; onClose: () => void }) {
  const { bridge, t } = useServices();
  const dialogs = useStoreSelector((state) => state.dialogs.length);
  const choose = useChoosePane();
  const [view, setView] = useState<CompareView | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  usePageOverlay(true);
  const refresh = useCallback(() => {
    setRefreshing(true);
    void bridge.call<CompareView | null>('compareView', { paneId, compareId }).then((value) => setView(value ?? { gone: true }), () => {}).finally(() => setRefreshing(false));
  }, [bridge, paneId, compareId]);
  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);
  const act = (childId: string | null, action: 'diff' | 'merge' | 'discard' | 'dismiss') => {
    setWorking(childId ?? '');
    void bridge.call('compareAction', { paneId, compareId, childId, action }).catch(() => {}).finally(() => {
      setWorking(null);
      if (action === 'dismiss') onClose(); else refresh();
    });
  };
  const columns = view?.children ?? [];
  const status = view && !view.gone ? [
    t('compare.view.base', { base: view.baseShort ?? '' }) + (view.baseBranch ? ' · ' + view.baseBranch : ''),
    view.testName ? t('compare.view.testName', { name: view.testName }) : null,
  ].filter(Boolean).join(' · ') : '';
  return createPortal(
    <DialogShell kind="panel" panel="compare" width={980} height={560} minWidth={560} minHeight={320} label={t('compare.view.title')} testId="compare-panel"
      layer="surface" top={dialogs === 0} icon={<Icon name="columns-3" size={18} weight={1.8} />} title={t('compare.view.title')}
      subtitle={view?.request ? view.request.split('\n').find((line) => line.trim()) : undefined}
      tools={<button type="button" className="icon-button" aria-label={t('compare.view.refresh')} title={t('compare.view.refresh')} disabled={refreshing} onClick={refresh}>
        <Icon name="refresh-cw" size={14} weight={2} /></button>}
      onCancel={onClose} closeTestId="compare-close" bodyClassName="compare-body" backdrop={{ 'data-page-overlay': '' }} status={status}
      buttons={view && !view.gone ? <button type="button" className="btn" title={t('compare.view.dismissHelp')} onClick={() => act(null, 'dismiss')}>{t('compare.view.dismiss')}</button> : undefined}>
      {!view ? <DialogLoading rows={2} /> : view.gone ? <div className="compare-unavailable" role="status">{t('compare.view.gone')}</div> : (
        <div className="compare-columns" role="list" style={{ gridTemplateColumns: `repeat(${Math.max(1, columns.length)}, minmax(200px, 1fr))` }}>
          {columns.map((column) => (
            <Column key={column.id} column={column} chosenTest={!!view.test} working={working !== null} act={(action) => act(column.id, action)}
              open={() => { choose(column.id); onClose(); }} />
          ))}
        </div>
      )}
    </DialogShell>,
    document.body,
  );
}

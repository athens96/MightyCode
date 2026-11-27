// MC-119: the sidebar's third tab (루프, ⌘3 / Ctrl+3), shown only when loopsEnabled.
// Groups loops by state (나를 기다림, 도는 중, 끝남); each row shows title, iteration n/max, cost meter, and state chip.
import { useState } from 'react';
import { useServices, useStoreSelector } from '../app/context';
import { Icon } from '../icons/Icon';
import { useCall, useMenu, type MenuItem } from '../shell/parts';
import { closeLoopDetail, openLoopCreate, openLoopDetail } from './events';
import {
  groupLoops, isFinished, loopGroupKeys, loopMatches, loopRowText, meterFraction, stateChip,
  type LoopGroup,
} from './model';
import type { LoopRecord } from './types';
import './loops.css';

const groupDotTone: Record<LoopGroup, string> = {
  waiting: 'wait',
  running: 'run',
  finished: 'done',
};

export function LoopTab({ search, selectedLoopId }: { search: string; selectedLoopId: string | null }) {
  const { t, store } = useServices();
  const call = useCall();
  const menu = useMenu();
  const loops = useStoreSelector((state) => state.loops.loops);
  const startPaneId = useStoreSelector((state) => {
    const snap = state.snapshot;
    if (!snap) return null;
    const active = snap.sessions.find((s) => s.id === (state.chosen ?? snap.activeSessionId) && s.kind === 'claude' && !s.parentSessionId);
    if (active) return active.id;
    const inWorkspace = snap.sessions.find((s) => s.workspaceId === snap.activeWorkspaceId && s.kind === 'claude' && !s.parentSessionId);
    if (inWorkspace) return inWorkspace.id;
    return snap.sessions.find((s) => s.kind === 'claude' && !s.parentSessionId)?.id ?? null;
  });
  const [folded, setFolded] = useState<string[]>([]);
  const toggleGroup = (key: string) => {
    setFolded((prev) => (prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key]));
  };

  const shown = loops.filter((loop) => loopMatches(loop, search));
  const groups = groupLoops(shown);

  const openPane = (paneId: string) => {
    closeLoopDetail();
    store.dispatch({ kind: 'choose', paneId });
    call('selectSession', { id: paneId });
  };

  const rowMenu = (loop: LoopRecord): MenuItem[] => {
    const items: MenuItem[] = [];
    if (loop.state === 'running') {
      items.push({ label: t('loop.ui.action.pause'), action: () => call('loopPause', { id: loop.id }) });
      items.push({ label: t('loop.ui.action.stop'), action: () => call('loopStop', { id: loop.id }) });
    } else if (loop.state === 'paused') {
      items.push({ label: t('loop.ui.action.resume'), action: () => call('loopResume', { id: loop.id }) });
      items.push({ label: t('loop.ui.action.stop'), action: () => call('loopStop', { id: loop.id }) });
    }
    items.push({ label: t('loop.ui.action.viewInPane'), action: () => openPane(loop.paneId) });
    if (isFinished(loop)) {
      items.push({ separator: true }, { label: t('loop.ui.action.forget'), action: () => call('loopForget', { id: loop.id }) });
    }
    return items;
  };

  const renderRow = (loop: LoopRecord) => {
    const chip = stateChip(loop, t);
    const text = loopRowText(loop, t);
    const finished = isFinished(loop);
    const share = meterFraction(loop.costUsd, loop.limits.maxCostUsd);
    const warn = share >= 0.8 && !finished;
    const selected = loop.id === selectedLoopId;
    return (
      <button
        key={loop.id}
        type="button"
        className={'sidebar-loop-row' + (selected ? ' selected' : '') + (finished ? ' finished' : '')}
        data-testid="sidebar-loop-row"
        data-loop-id={loop.id}
        aria-current={selected ? 'true' : undefined}
        title={`${loop.title}\n${text.detail}\n${text.cost}`}
        onClick={() => openLoopDetail(loop.id)}
        onContextMenu={(event) => menu.atPointer(event, rowMenu(loop))}
      >
        <span className="sidebar-loop-row-head">
          <span className={'loop-dot tone-' + chip.tone} aria-hidden="true" />
          <span className="sidebar-loop-row-title">{loop.title}</span>
          <span className={'loop-state-chip tone-' + chip.tone} data-testid="sidebar-loop-chip">{chip.label}</span>
        </span>
        <span className="sidebar-loop-row-sub">
          <span className="sidebar-loop-row-detail" data-testid="sidebar-loop-detail">{text.detail}</span>
        </span>
        {loop.limits.maxCostUsd > 0 && (
          <span className="sidebar-loop-row-meter">
            <span className={'loop-meter' + (warn ? ' warn' : finished && loop.state === 'passed' ? ' done' : '')} aria-hidden="true">
              <span className="loop-meter-fill" style={{ width: `${Math.round(share * 100)}%` }} />
            </span>
            <span className="sidebar-loop-row-cost">{text.cost}</span>
          </span>
        )}
      </button>
    );
  };

  return (
    <div className="sidebar-loops" role="tabpanel" data-testid="sidebar-loops">
      <button
        type="button"
        className="sidebar-list-add"
        data-testid="sidebar-start-loop"
        disabled={!startPaneId}
        title={startPaneId ? t('loop.ui.startButton') : t('loop.ui.noPaneToStart')}
        onClick={() => { if (startPaneId) openLoopCreate(startPaneId, ''); }}
      >
        <Icon name="plus" size={13} />{t('loop.ui.startButton')}
      </button>
      {groups.map(({ group, loops: items }) => {
        const isFolded = folded.includes(group);
        return (
          <div key={group} className="sidebar-loop-group" data-testid={'sidebar-loop-group-' + group}>
            <button
              type="button"
              className="sidebar-loop-group-header"
              aria-expanded={!isFolded}
              onClick={() => toggleGroup(group)}
            >
              <Icon name={isFolded ? 'chevron-right' : 'chevron-down'} size={10} weight={2.6} />
              <span className={'loop-dot tone-' + groupDotTone[group]} aria-hidden="true" />
              <span>{t(loopGroupKeys[group])}</span>
              <span className="sidebar-section-count">{items.length}</span>
            </button>
            {!isFolded && <div className="sidebar-loop-group-rows">{items.map(renderRow)}</div>}
          </div>
        );
      })}
      {shown.length === 0 && (
        <div className="sidebar-bots-empty" data-testid="sidebar-loops-empty">
          <span>{t(search.trim().length > 0 ? 'sidebar.noSearchResults' : 'loop.ui.empty')}</span>
        </div>
      )}
    </div>
  );
}

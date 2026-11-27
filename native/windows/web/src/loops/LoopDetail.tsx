// MC-119: main-area loop detail view (mockup section A): header, stats, criteria chips (L1–L4), guard flags,
// and iteration timeline with 창에서 보기 revealing each iteration's runId in its pane.
import { useEffect, useState } from 'react';
import { useServices, useStoreSelector } from '../app/context';
import { SidebarToggle } from '../dashboard/Dashboard';
import { Icon } from '../icons/Icon';
import { clockOf } from '../shell/reportRules';
import { closeLoopDetail } from './events';
import {
  criterionTitle, elapsedMinutes, failedCriterionLabel, formatDurationMs, formatLimitUsd, formatMinutes,
  formatUsd, guardFlagWords, hasGuardFlags, isFinished, iterationProgress, meterFraction, phaseWords,
  stateChip, stopReasonWords,
} from './model';
import type { CriterionStatus, IterationOutcome, LoopIteration, LoopRecord } from './types';
import './loops.css';

const criterionMark: Record<CriterionStatus, string> = {
  passed: '✓',
  failed: '✗',
  pending: '◌',
};

const outcomeTone: Record<IterationOutcome, 'run' | 'done' | 'err' | 'stop'> = {
  running: 'run',
  passed: 'done',
  failed: 'err',
  stopped: 'stop',
};

const outcomeKeys: Record<IterationOutcome, string> = {
  running: 'loop.ui.outcome.running',
  passed: 'loop.ui.outcome.passed',
  failed: 'loop.ui.outcome.failed',
  stopped: 'loop.ui.outcome.stopped',
};

export function LoopDetail({ loop }: { loop: LoopRecord }) {
  const { t, bridge, store } = useServices();
  const [busy, setBusy] = useState<'pause' | 'resume' | 'stop' | 'forget' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const paneTitle = useStoreSelector((state) => state.snapshot?.sessions.find((s) => s.id === loop.paneId)?.title);

  // Refresh elapsed time while the loop is running.
  useEffect(() => {
    if (loop.state !== 'running') return;
    const timer = window.setInterval(() => setNow(Date.now()), 15_000);
    return () => window.clearInterval(timer);
  }, [loop.state]);

  // Clear stale action error when switching to another loop.
  useEffect(() => { setError(null); }, [loop.id]);

  const runAction = async (kind: 'pause' | 'resume' | 'stop' | 'forget', method: string) => {
    if (busy) return;
    setBusy(kind);
    setError(null);
    try {
      await bridge.call(method, { id: loop.id });
      if (kind === 'forget') closeLoopDetail();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  };

  // Reveals `runId` in its pane the exact way the notices box (ReportBox.tsx) does.
  const viewInPane = (runId?: string | null) => {
    if (!store.getState().snapshot?.sessions.some((s) => s.id === loop.paneId)) {
      setError(t('loop.ui.paneGone'));
      return;
    }
    closeLoopDetail();
    store.dispatch({ kind: 'choose', paneId: loop.paneId });
    void bridge.call('selectSession', { id: loop.paneId }).catch(() => {});
    store.dispatch({ kind: 'aim', paneId: loop.paneId, ...(runId ? { runId } : {}) });
  };

  const chip = stateChip(loop, t);
  const { current, max } = iterationProgress(loop);
  const iterShare = meterFraction(current, max);
  const costShare = meterFraction(loop.costUsd, loop.limits.maxCostUsd);
  const elapsed = elapsedMinutes(loop.startedAt, loop.finishedAt, now);
  const finished = isFinished(loop);
  const guardFlags = loop.guard?.flags ?? [];
  const touched = loop.guard?.touched ?? [];
  const showGuardCard = hasGuardFlags(loop) || touched.length > 0;
  const phaseLabel = loop.state === 'running' ? phaseWords(loop.phase, t) : '';
  const stopLabel = stopReasonWords(loop.stopReason, t);

  // Newest iteration first, matching the mockup's timeline.
  const timeline = [...loop.iterations].sort((a, b) => b.n - a.n);

  const kindLabel = (kind: LoopIteration['kind']) => {
    if (kind === 'build') return t('loop.ui.kind.build');
    if (kind === 'repair') return t('loop.ui.kind.repair');
    return kind;
  };

  const iterationDuration = (it: LoopIteration): string | null => {
    if (!it.startedAt) return null;
    const start = Date.parse(it.startedAt);
    const end = it.finishedAt ? Date.parse(it.finishedAt) : it.outcome === 'running' ? now : NaN;
    if (Number.isNaN(start) || Number.isNaN(end) || end < start) return null;
    return formatDurationMs(end - start);
  };

  return (
    <section className="loop-detail" data-testid="loop-detail" data-loop-id={loop.id}>
      <header className="loop-detail-header">
        <div className="loop-detail-title-row">
          <SidebarToggle />
          <span className={'loop-dot tone-' + chip.tone} aria-hidden="true" />
          <h2 className="loop-detail-title" title={loop.title}>{loop.title}</h2>
          <span className={'loop-state-chip tone-' + chip.tone} data-testid="loop-detail-state">
            {phaseLabel ? `${chip.label} · ${phaseLabel}` : chip.label}
          </span>
          <span className="spacer" />
          <div className="loop-detail-actions">
            {loop.state === 'running' && (
              <>
                <button type="button" className="loop-btn" data-testid="loop-pause" disabled={busy !== null}
                  title={t('loop.ui.action.pauseHelp')} onClick={() => void runAction('pause', 'loopPause')}>
                  {t('loop.ui.action.pause')}
                </button>
                <button type="button" className="loop-btn danger" data-testid="loop-stop" disabled={busy !== null}
                  title={t('loop.ui.action.stopHelp')} onClick={() => void runAction('stop', 'loopStop')}>
                  {t('loop.ui.action.stop')}
                </button>
              </>
            )}
            {loop.state === 'paused' && (
              <>
                <button type="button" className="loop-btn primary" data-testid="loop-resume" disabled={busy !== null}
                  title={t('loop.ui.action.resumeHelp')} onClick={() => void runAction('resume', 'loopResume')}>
                  {t('loop.ui.action.resume')}
                </button>
                <button type="button" className="loop-btn danger" data-testid="loop-stop" disabled={busy !== null}
                  title={t('loop.ui.action.stopHelp')} onClick={() => void runAction('stop', 'loopStop')}>
                  {t('loop.ui.action.stop')}
                </button>
              </>
            )}
            {finished && (
              <button type="button" className="loop-btn" data-testid="loop-forget" disabled={busy !== null}
                onClick={() => void runAction('forget', 'loopForget')}>
                {t('loop.ui.action.forget')}
              </button>
            )}
            <button type="button" className="loop-btn ghost" data-testid="loop-detail-close"
              title={t('loop.ui.action.closeDetail')} aria-label={t('loop.ui.action.closeDetail')} onClick={closeLoopDetail}>
              <Icon name="x" size={14} weight={2} />
            </button>
          </div>
        </div>

        <p className="loop-detail-goal" data-testid="loop-detail-goal">{loop.goal}</p>

        <div className="loop-detail-meta">
          {paneTitle && (
            <button type="button" className="link-button" onClick={() => viewInPane(null)}>
              {t('loop.ui.detail.pane', { pane: paneTitle })}
            </button>
          )}
          {loop.baseCommit && (
            <span className="mono">{t('loop.ui.detail.baseCommit', { sha: loop.baseCommit.slice(0, 7) })}</span>
          )}
          {stopLabel && (
            <span className={'loop-state-chip tone-' + chip.tone}>{stopLabel}</span>
          )}
        </div>

        {error && <div className="loop-detail-error" role="alert" data-testid="loop-detail-error">{error}</div>}

        <div className="loop-detail-stats">
          <div className="loop-stat">
            <span className="loop-stat-label">{t('loop.ui.stat.iterations')}</span>
            <div className="loop-stat-value">
              <span>{current} / {max}</span>
              <span className="loop-meter" aria-hidden="true">
                <span className="loop-meter-fill" style={{ width: `${Math.round(iterShare * 100)}%` }} />
              </span>
            </div>
          </div>
          <div className="loop-stat">
            <span className="loop-stat-label">{t('loop.ui.stat.cost')}</span>
            <div className="loop-stat-value">
              <span>{formatUsd(loop.costUsd)} / {formatLimitUsd(loop.limits.maxCostUsd)}</span>
              <span className={'loop-meter' + (costShare >= 0.8 ? ' warn' : '')} aria-hidden="true">
                <span className="loop-meter-fill" style={{ width: `${Math.round(costShare * 100)}%` }} />
              </span>
            </div>
          </div>
          <div className="loop-stat">
            <span className="loop-stat-label">{t('loop.ui.stat.elapsed')}</span>
            <div className="loop-stat-value">
              <span>{formatMinutes(elapsed, t)} / {formatMinutes(loop.limits.maxMinutes, t)}</span>
            </div>
          </div>
          <div className="loop-stat">
            <span className="loop-stat-label">{t('loop.ui.stat.guard')}</span>
            <div className="loop-stat-value">
              <span className={guardFlags.length === 0 ? 'tone-text-done' : 'tone-text-wait'}>
                {guardFlags.length === 0 ? t('loop.ui.guard.clean') : t('loop.ui.guard.flagCount', { count: guardFlags.length })}
              </span>
            </div>
          </div>
        </div>
      </header>

      <div className="loop-criteria-bar" data-testid="loop-criteria">
        <span className="loop-criteria-label">{t('loop.ui.criteria.label')}</span>
        {loop.criteria.map((c) => {
          const ev = c.evidence;
          const dur = formatDurationMs(ev?.durationMs);
          const tip = [
            criterionTitle(c),
            c.check.command ? `$ ${c.check.command}` : '',
            ev?.summary ?? '',
            dur ?? '',
          ].filter(Boolean).join('\n');
          return (
            <span key={c.id} className={'loop-criterion-chip status-' + c.status} data-testid="loop-criterion-chip" title={tip}>
              <span aria-hidden="true">{criterionMark[c.status] ?? '◌'}</span>
              <span>{criterionTitle(c)}</span>
              <span className="loop-criterion-level">{c.level}</span>
              {c.status === 'failed' && ev?.summary && <span className="loop-criterion-evidence">· {ev.summary}</span>}
            </span>
          );
        })}
      </div>

      <div className="loop-detail-body">
        {showGuardCard && (
          <article className="loop-guard-card" data-testid="loop-guard-card">
            <div className="loop-guard-head">
              <span className="loop-dot tone-wait" aria-hidden="true" />
              <span>{t('loop.ui.guard.title')}</span>
              <span className="spacer" />
              <span className="loop-state-chip tone-wait">{t('loop.ui.guard.flagCount', { count: Math.max(guardFlags.length, touched.length > 0 ? 1 : 0) })}</span>
            </div>
            {guardFlags.length > 0 && (
              <div className="loop-guard-flags">
                {guardFlags.map((flag) => (
                  <span key={flag} className="loop-state-chip tone-wait" data-testid="loop-guard-flag">{guardFlagWords(flag, t)}</span>
                ))}
              </div>
            )}
            <div className="loop-guard-meta">
              <span>{t('loop.ui.guard.protectedFiles', { count: loop.guard.protectedFiles })}</span>
              <span>{t('loop.ui.guard.tests', { start: loop.guard.testsAtStart, now: loop.guard.testsNow })}</span>
              {loop.guard.skipsAdded > 0 && <span>{t('loop.ui.guard.skips', { count: loop.guard.skipsAdded })}</span>}
            </div>
            {touched.length > 0 && (
              <div className="loop-guard-touched">
                <span>{t('loop.ui.guard.touched')}:</span>
                {touched.map((file) => <span key={file} className="loop-guard-path">{file}</span>)}
              </div>
            )}
          </article>
        )}

        {timeline.length === 0 ? (
          <div className="loop-timeline-empty">{t('loop.ui.timeline.empty')}</div>
        ) : (
          timeline.map((it) => {
            const tone = outcomeTone[it.outcome] ?? 'stop';
            const dur = iterationDuration(it);
            return (
              <article key={it.n} className={'loop-iteration' + (it.outcome === 'running' ? ' current' : '')} data-testid="loop-iteration">
                <div className="loop-iteration-head">
                  <span className={'loop-dot tone-' + tone} aria-hidden="true" />
                  <span className="loop-iteration-n">{t('loop.ui.iterationNumber', { n: it.n })}</span>
                  <span className="loop-iteration-kind">{kindLabel(it.kind)}</span>
                  <span className={'loop-state-chip tone-' + tone}>{t(outcomeKeys[it.outcome] ?? 'loop.ui.outcome.stopped')}</span>
                  {dur && <span className="loop-iteration-meta">{dur}</span>}
                  <span className="loop-iteration-meta mono">{formatUsd(it.costUsd)}</span>
                  <span className="spacer" />
                  <button
                    type="button"
                    className="link-button"
                    data-testid="loop-iteration-reveal"
                    onClick={() => viewInPane(it.runId)}
                  >
                    {t('loop.ui.action.viewInPane')} →
                  </button>
                </div>
                {(it.changedFiles > 0 || it.added > 0 || it.removed > 0 || it.failed.length > 0) && (
                  <div className="loop-iteration-body">
                    {(it.changedFiles > 0 || it.added > 0 || it.removed > 0) && (
                      <>
                        <span>{t('loop.ui.iteration.changedFiles', { count: it.changedFiles })}</span>
                        <span className="loop-diff-add">+{it.added}</span>
                        <span className="loop-diff-del">−{it.removed}</span>
                      </>
                    )}
                    {it.failed.map((id) => (
                      <span key={id} className="loop-criterion-chip status-failed">
                        ✗ {failedCriterionLabel(id, loop.criteria)}
                      </span>
                    ))}
                  </div>
                )}
              </article>
            );
          })
        )}

        <div className="loop-timeline-start">
          <span>{t('loop.ui.timeline.started', { at: clockOf(loop.startedAt) || loop.startedAt })}</span>
          {loop.baseCommit && <span className="mono">{t('loop.ui.detail.baseCommit', { sha: loop.baseCommit.slice(0, 7) })}</span>}
        </div>
      </div>
    </section>
  );
}

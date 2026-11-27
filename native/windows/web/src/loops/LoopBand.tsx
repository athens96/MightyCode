// MC-119: one-line band at the top of a pane driven by a running or paused loop.
// Derived from the loops list (paneId match + state running|paused; no pane.loopId field).
import { useEffect, useState } from 'react';
import { shallowEqual, useServices, useStoreSelector } from '../app/context';
import { Icon } from '../icons/Icon';
import { openLoopDetail } from './events';
import { activeLoopForPane, criterionTitle, firstFailingCriterion, guardFlagWords, hasGuardFlags, iterationProgress } from './model';
import './loops.css';

export function LoopBand({ paneId }: { paneId: string }) {
  const { t, bridge } = useServices();
  const { enabled, loop } = useStoreSelector(
    (state) => ({
      enabled: state.loops.enabled,
      loop: activeLoopForPane(state.loops.loops, paneId),
    }),
    shallowEqual,
  );
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A new loop in the pane starts with a clean band (no earlier loop's error or pending stop).
  const loopId = loop?.id;
  useEffect(() => {
    setError(null);
    setStopping(false);
  }, [loopId]);

  if (!enabled || !loop) return null;

  const { current, max } = iterationProgress(loop);
  const failing = firstFailingCriterion(loop);
  const guarded = hasGuardFlags(loop);

  const stop = async () => {
    if (stopping) return;
    setStopping(true);
    setError(null);
    try {
      await bridge.call('loopStop', { id: loop.id });
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setStopping(false);
    }
  };

  return (
    <div className={'loop-band' + (loop.state === 'paused' ? ' paused' : '')} data-testid="pane-loop-band" data-loop-id={loop.id}>
      <Icon name="refresh-cw" size={12} weight={2.2} className="loop-band-icon" />
      <span className="loop-band-label">{t('loop.ui.band.label')}</span>
      <span className="loop-band-sep" aria-hidden="true">·</span>
      <span className="loop-band-title" title={loop.title}>{loop.title}</span>
      <span className="loop-band-sep" aria-hidden="true">·</span>
      <span className="loop-band-iter">{t('loop.ui.iterationRatio', { n: current, max })}</span>
      {failing ? (
        <>
          <span className="loop-band-sep" aria-hidden="true">·</span>
          <span className="loop-band-fail" data-testid="pane-loop-band-fail" title={criterionTitle(failing)}>
            ✗ {criterionTitle(failing)}
          </span>
        </>
      ) : guarded ? (
        <>
          <span className="loop-band-sep" aria-hidden="true">·</span>
          <span className="loop-band-fail guard" data-testid="pane-loop-band-guard">
            {guardFlagWords(loop.guard.flags[0]!, t)}
          </span>
        </>
      ) : null}
      <span className="loop-band-actions">
        {error && <span className="loop-band-err" role="alert">{error}</span>}
        <button
          type="button"
          className="link-button"
          data-testid="pane-loop-band-view"
          onClick={() => openLoopDetail(loop.id, true)}
        >
          {t('loop.ui.band.view')} →
        </button>
        <button
          type="button"
          className="loop-btn danger loop-band-stop"
          data-testid="pane-loop-band-stop"
          disabled={stopping}
          onClick={() => void stop()}
        >
          {t('loop.ui.action.stop')}
        </button>
      </span>
    </div>
  );
}

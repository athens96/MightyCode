// MC-119: the loop create sheet (mockup 공통 ②), opened by the send button's ↻ 될 때까지 반복… entry,
// ⌥⌘Enter / Ctrl+Alt+Enter, or the sidebar 루프 tab's + 루프 시작 button.
import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import { openLoopCreateEvent, openLoopDetail, type OpenLoopCreateDetail } from './events';
import {
  addCommandCriterion, buildLoopStartArgs, editDraftCriterionText, formFromDraft,
  removeDraftCriterion, toggleDraftCriterion, type LoopDraftForm,
} from './model';
import type { CriterionLevel, LoopRecord } from './types';
import './loops.css';

const levels: readonly CriterionLevel[] = ['L1', 'L2', 'L3', 'L4'];

/** Mounted once in Shell: listens for openLoopCreateEvent and shows the sheet. */
export function LoopCreateHost() {
  const [open, setOpen] = useState<OpenLoopCreateDetail | null>(null);
  useEffect(() => {
    const show = (event: Event) => {
      const detail = (event as CustomEvent<OpenLoopCreateDetail | undefined>).detail;
      if (detail?.paneId) setOpen({ paneId: detail.paneId, goal: detail.goal ?? '' });
    };
    window.addEventListener(openLoopCreateEvent, show);
    return () => window.removeEventListener(openLoopCreateEvent, show);
  }, []);
  return open ? <LoopCreateSheet paneId={open.paneId} initialGoal={open.goal ?? ''} onClose={() => setOpen(null)} /> : null;
}

export function LoopCreateSheet({ paneId, initialGoal, onClose }: { paneId: string; initialGoal: string; onClose: () => void }) {
  const { t, bridge, store } = useServices();
  usePageOverlay(true);
  const [form, setForm] = useState<LoopDraftForm | null>(null);
  const [cmdInput, setCmdInput] = useState('');
  const [cmdLevel, setCmdLevel] = useState<CriterionLevel>('L2');
  const [cmdError, setCmdError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draftFailed, setDraftFailed] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    void bridge.call<LoopRecord>('loopDraft', { paneId, goal: initialGoal }).then(
      (draft) => {
        if (!live) return;
        setForm(formFromDraft(draft, initialGoal));
      },
      (failure: unknown) => {
        if (!live) return;
        setDraftFailed(failure instanceof Error ? failure.message : String(failure));
      },
    );
    return () => { live = false; };
  }, [bridge, paneId, initialGoal]);

  const addCommand = () => {
    if (!form) return;
    const res = addCommandCriterion(form, cmdInput, cmdLevel);
    if (!res.ok) {
      setCmdError(t(res.errorKey));
      return;
    }
    setForm(res.form);
    setCmdInput('');
    setCmdError(null);
    setError(null);
  };

  const start = async () => {
    if (!form || busy) return;
    const built = buildLoopStartArgs(paneId, form);
    if (!built.ok) {
      setError(t(built.errorKey));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const started = await bridge.call<LoopRecord>('loopStart', built.args);
      store.dispatch({ kind: 'fill', paneId, text: '', append: false });
      onClose();
      if (started?.id) openLoopDetail(started.id, true);
    } catch (failure) {
      // Show the host's error string verbatim as it comes.
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  const body = draftFailed ? (
    <div className="loop-create-error" role="alert" data-testid="loop-create-error">{draftFailed}</div>
  ) : !form ? (
    <DialogLoading rows={3} />
  ) : (
    <div className="loop-create" data-testid="loop-create-form">
      <div className="loop-create-field">
        <label className="loop-create-label" htmlFor="loop-create-goal">{t('loop.ui.create.goal')}</label>
        <textarea
          id="loop-create-goal"
          className="loop-create-textarea"
          data-testid="loop-create-goal"
          data-autofocus
          value={form.goal}
          placeholder={t('loop.ui.create.goalPlaceholder')}
          disabled={busy}
          onChange={(event) => { setForm({ ...form, goal: event.target.value }); setError(null); }}
        />
      </div>

      <div className="loop-create-field">
        <span className="loop-create-label">{t('loop.ui.create.criteria')}</span>
        <div className="loop-create-criteria">
          {form.criteria.map((c) => (
            <div key={c.id} className={'loop-create-criterion' + (c.enabled ? '' : ' off')} data-testid="loop-create-criterion">
              <input
                type="checkbox"
                checked={c.enabled}
                disabled={busy}
                aria-label={c.text}
                onChange={() => { setForm(toggleDraftCriterion(form, c.id)); setError(null); }}
              />
              <div className="loop-create-criterion-main">
                <input
                  type="text"
                  className="loop-create-criterion-text"
                  value={c.text}
                  disabled={busy}
                  aria-label={t('loop.ui.create.criterionText')}
                  onChange={(event) => setForm(editDraftCriterionText(form, c.id, event.target.value))}
                />
                <span className="loop-create-criterion-sub">
                  <span className="loop-criterion-level">{c.level}</span>
                  <span>{c.check.kind}</span>
                  {c.check.command && <span className="mono loop-create-criterion-cmd">$ {c.check.command}</span>}
                </span>
              </div>
              {c.custom && (
                <button
                  type="button"
                  className="icon-button"
                  disabled={busy}
                  title={t('loop.ui.create.removeCriterion')}
                  aria-label={t('loop.ui.create.removeCriterion')}
                  onClick={() => setForm(removeDraftCriterion(form, c.id))}
                >
                  <Icon name="x" size={12} />
                </button>
              )}
            </div>
          ))}
        </div>
        <div className="loop-create-add-row">
          <input
            type="text"
            className="dialog-field mono"
            data-testid="loop-create-cmd-input"
            value={cmdInput}
            disabled={busy}
            placeholder={t('loop.ui.create.addCommandPlaceholder')}
            aria-label={t('loop.ui.create.addCommand')}
            onChange={(event) => { setCmdInput(event.target.value); setCmdError(null); }}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                event.preventDefault();
                addCommand();
              }
            }}
          />
          <select
            className="dialog-field mono"
            value={cmdLevel}
            disabled={busy}
            aria-label={t('loop.ui.create.level')}
            onChange={(event) => setCmdLevel(event.target.value as CriterionLevel)}
          >
            {levels.map((lv) => <option key={lv} value={lv}>{lv}</option>)}
          </select>
          <button type="button" className="btn" data-testid="loop-create-cmd-add" disabled={busy || cmdInput.trim().length === 0} onClick={addCommand}>
            {t('loop.ui.create.addCommandButton')}
          </button>
        </div>
        {cmdError && <div className="loop-create-error" role="alert">{cmdError}</div>}
      </div>

      <div className="loop-create-field">
        <span className="loop-create-label">{t('loop.ui.create.limits')}</span>
        <div className="loop-create-limits">
          <label className="loop-create-limit">
            <span>{t('loop.ui.create.maxIterations')}</span>
            <input
              className="dialog-field"
              inputMode="numeric"
              data-testid="loop-create-max-iterations"
              value={form.maxIterations}
              disabled={busy}
              onChange={(event) => setForm({ ...form, maxIterations: event.target.value })}
            />
          </label>
          <label className="loop-create-limit">
            <span>{t('loop.ui.create.maxMinutes')}</span>
            <input
              className="dialog-field"
              inputMode="numeric"
              data-testid="loop-create-max-minutes"
              value={form.maxMinutes}
              disabled={busy}
              onChange={(event) => setForm({ ...form, maxMinutes: event.target.value })}
            />
          </label>
          <label className="loop-create-limit">
            <span>{t('loop.ui.create.maxCost')}</span>
            <input
              className="dialog-field"
              inputMode="decimal"
              data-testid="loop-create-max-cost"
              value={form.maxCostUsd}
              disabled={busy}
              onChange={(event) => setForm({ ...form, maxCostUsd: event.target.value })}
            />
          </label>
        </div>
      </div>

      <p className="loop-create-hint">{t('loop.ui.create.guardNote')}</p>
      {error && <div className="loop-create-error" role="alert" data-testid="loop-create-error">{error}</div>}
    </div>
  );

  const buttons = (
    <>
      <button type="button" className="btn" disabled={busy} onClick={onClose}>{t('loop.ui.create.cancel')}</button>
      {!draftFailed && (
        <button type="button" className="btn primary" data-testid="loop-create-start" disabled={busy || !form} onClick={() => void start()}>
          {busy ? t('loop.ui.create.starting') : t('loop.ui.create.start')}
        </button>
      )}
    </>
  );

  return createPortal(
    <DialogShell
      kind="sheet"
      width={560}
      closable
      label={t('loop.ui.create.title')}
      title={t('loop.ui.create.title')}
      icon={<Icon name="refresh-cw" size={16} weight={2} />}
      busy={busy}
      onCancel={onClose}
      testId="loop-create-sheet"
      backdrop={{ 'data-page-overlay': '' }}
      buttons={buttons}
    >
      {body}
    </DialogShell>,
    document.body,
  );
}

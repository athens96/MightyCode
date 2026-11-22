// 비교 실행's picker (MC-29): the composer's 비교 실행… opens it with the editor's words. Rows of agent and model (up to the room
// the host allows, four at most; the same agent may come again), the base commit (HEAD first), an optional test run in each child
// once it reports, and the budget line. Start sends it all to `compareStart`; the composer is emptied and the compare view opens.
import { useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import { budgetCheck, firstRows, nextRow, usd, type CompareOptions, type CompareRowPick, type CompareStartAnswer } from './compare';
import './compare.css';

export function ComparePicker({ paneId, text, onClose, onStarted }: { paneId: string; text: string; onClose: () => void; onStarted: (compareId: string) => void }) {
  const { bridge, store, t } = useServices();
  const dialogs = useStoreSelector((state) => state.dialogs.length);
  const [options, setOptions] = useState<CompareOptions | null>(null);
  const [rows, setRows] = useState<CompareRowPick[]>([]);
  const [base, setBase] = useState<string | null>(null);
  const [test, setTest] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [budgetAsked, setBudgetAsked] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  usePageOverlay(true);
  useEffect(() => {
    let live = true;
    void bridge.call<CompareOptions | null>('compareOptions', { paneId }).then((value) => {
      if (!live) return;
      const read = value ?? { available: false, reason: t('compare.reason.notAgent') };
      setOptions(read);
      setRows(firstRows(read));
      setBase(read.bases?.[0]?.sha ?? null);
    }, (failure: unknown) => { if (live) setOptions({ available: false, reason: failure instanceof Error ? failure.message : String(failure) }); });
    return () => { live = false; };
  }, [bridge, paneId, t]);
  const request = text.trim();
  const check = useMemo(() => (options ? budgetCheck(options, rows) : null), [options, rows]);
  const max = options?.max ?? 0;
  const ready = !!options?.available && request.length > 0 && rows.length >= 2 && rows.length <= max && !busy;
  const setRow = (index: number, row: CompareRowPick) => setRows(rows.map((r, i) => (i === index ? row : r)));
  const start = () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    void bridge.call<CompareStartAnswer | null>('compareStart', {
      paneId, text: request, rows, base, test, budgetConfirmed: !!check?.over || budgetAsked !== null,
    }).then((answer) => {
      if (answer?.ok && answer.compareId) {
        store.dispatch({ kind: 'fill', paneId, text: '', append: false });
        onStarted(answer.compareId);
        return;
      }
      if (answer?.budget) { setBudgetAsked(answer.budget); return; }
      setError(answer?.error ?? t('compare.error.noneStarted'));
    }, (failure: unknown) => setError(failure instanceof Error ? failure.message : String(failure))).finally(() => setBusy(false));
  };
  const agents = options?.agents ?? [];
  const tests = options?.tests ?? [];
  const warn = check?.over || budgetAsked !== null;
  const body = !options ? <DialogLoading rows={2} /> : !options.available ? (
    <div className="compare-unavailable" role="status" data-testid="compare-unavailable">{t('compare.picker.unavailable', { reason: options.reason ?? '' })}</div>
  ) : (
    <div className="compare-picker">
      <section className="compare-section">
        <div className="compare-label">{t('compare.picker.request')}</div>
        {request ? <div className="compare-request" title={request}>{request}</div> : <div className="compare-note tone-text-err">{t('compare.picker.noRequest')}</div>}
      </section>
      <section className="compare-section">
        <div className="compare-label">{t('compare.picker.agents')}<span className="compare-hint">{t('compare.picker.max', { count: max })}</span></div>
        {rows.map((row, index) => {
          const agent = agents.find((a) => a.agent === row.agent);
          return (
            <div key={index} className="compare-row" data-testid={'compare-row-' + index}>
              <span className="compare-row-number">{index + 1}</span>
              <select className="dialog-field compare-agent" aria-label={t('compare.picker.agent')} value={row.agent}
                onChange={(event) => setRow(index, { agent: event.target.value, model: null })}>
                {agents.map((a) => <option key={a.agent} value={a.agent}>{a.label}</option>)}
              </select>
              <select className="dialog-field compare-model" aria-label={t('compare.picker.model')} value={row.model ?? ''}
                onChange={(event) => setRow(index, { agent: row.agent, model: event.target.value || null })}>
                <option value="">{t('compare.picker.defaultModel')}</option>
                {(agent?.models ?? []).map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
              </select>
              <button type="button" className="btn icon" aria-label={t('compare.picker.remove')} title={t('compare.picker.remove')} disabled={rows.length <= 2}
                onClick={() => setRows(rows.filter((_, i) => i !== index))}><Icon name="x" size={13} weight={2.2} /></button>
            </div>
          );
        })}
        <button type="button" className="btn text compare-add" data-testid="compare-add" disabled={rows.length >= max}
          onClick={() => { const next = nextRow(options, rows); if (next) setRows([...rows, next]); }}>
          <Icon name="plus" size={12} weight={2.4} />{t('compare.picker.add')}
        </button>
      </section>
      <section className="compare-section compare-two">
        <label className="compare-field">
          <span className="compare-label">{t('compare.picker.base')}</span>
          <select className="dialog-field mono" value={base ?? ''} onChange={(event) => setBase(event.target.value || null)} data-testid="compare-base">
            {(options.bases ?? []).map((b) => (
              <option key={b.sha} value={b.sha}>{(b.head ? t('compare.picker.head') + ' · ' : '') + b.short + ' ' + b.subject}</option>
            ))}
          </select>
        </label>
        <label className="compare-field">
          <span className="compare-label">{t('compare.picker.test')}</span>
          <select className="dialog-field" value={test ?? ''} onChange={(event) => setTest(event.target.value || null)} data-testid="compare-test" disabled={tests.length === 0}>
            <option value="">{t('compare.picker.noTest')}</option>
            {tests.map((task) => <option key={task.id} value={task.id} title={task.command}>{task.name + ' · ' + task.source}</option>)}
          </select>
          {test && !options.testsApproved && <span className="compare-note tone-text-wait">{t('compare.test.notApproved')}</span>}
        </label>
      </section>
      {check && (
        <div className={'compare-budget' + (warn ? ' warn' : '')} title={t('compare.picker.estimateHelp')} data-testid="compare-budget">
          <Icon name={warn ? 'triangle-alert' : 'info'} size={13} weight={2} />
          <span>{budgetAsked ?? (check.over
            ? t('compare.budget.warn', { spent: usd(check.spent), estimate: usd(check.estimate), budget: usd(check.budget ?? 0) })
            : t('compare.picker.estimate', { estimate: usd(check.estimate) }))}</span>
        </div>
      )}
      {error && <div className="compare-note tone-text-err" role="alert">{error}</div>}
    </div>
  );
  return createPortal(
    <DialogShell kind="panel" panel="compare-picker" width={620} height={520} minWidth={460} minHeight={320} label={t('compare.picker.title')} testId="compare-picker"
      layer="surface" top={dialogs === 0} icon={<Icon name="columns-3" size={18} weight={1.8} />} title={t('compare.picker.title')}
      onCancel={onClose} closeTestId="compare-picker-close" backdrop={{ 'data-page-overlay': '' }} busy={busy}
      buttons={<>
        <button type="button" className="btn" onClick={onClose}>{t('resume.cancel')}</button>
        <button type="button" className={'btn ' + (warn ? 'danger' : 'primary')} data-testid="compare-start" disabled={!ready} onClick={start}>
          {t(warn ? 'compare.picker.startAnyway' : 'compare.picker.start')}
        </button>
      </>}>
      {body}
    </DialogShell>,
    document.body,
  );
}

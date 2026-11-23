// MC-34: a workspace's sandbox settings (sandbox.rs `sandboxGet` / `sandboxSave`): the switch (an SSH or remote workspace says the
// sandbox does not apply), the network allow list, the instruction files' read-only switch and more read-only paths, the reviewer
// switch with its agent and model, and what the sandbox refused lately in the workspace's panes. MC-42: on Windows the box is marked
// experimental, says what is enforced and what is only advisory, lists the changes to the person's folders before the sandbox turns
// on (approved with 승인하고 켜기; undone when it is turned off), and offers to undo every change (`sandboxUndoAll`).
import { useEffect, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import { providerLabel } from '../conversation/format';
import {
  GUARANTEE_KEYS, KIND_KEYS, LEVEL_KEYS, OP_KEYS, groupChanges, keyOf, levelTone, linesOf, pendingChanges, sandboxNotice,
  type SandboxReport, type SandboxStatus,
} from './sandboxModel';
import './SandboxPanel.css';

export interface SandboxPanelProps {
  open: boolean;
  workspaceId: string | null;
  onClose: () => void;
}

function portal(node: ReactNode) {
  return typeof document !== 'undefined' && document.body ? createPortal(node, document.body) : node;
}

export function SandboxPanel({ open, workspaceId, onClose }: SandboxPanelProps) {
  const { bridge, t } = useServices();
  usePageOverlay(open);
  const [status, setStatus] = useState<SandboxStatus | null>(null);
  const [hosts, setHosts] = useState('');
  const [paths, setPaths] = useState('');
  const [form, setForm] = useState<Partial<SandboxStatus>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [asking, setAsking] = useState(false);
  const [report, setReport] = useState<SandboxReport | null>(null);
  const [undoAsk, setUndoAsk] = useState(false);

  const take = (next: SandboxStatus) => {
    setStatus(next);
    setForm(next);
    setHosts(next.allowedHosts.join('\n'));
    setPaths(next.readOnlyPaths.join('\n'));
  };

  useEffect(() => {
    if (!open) return;
    setError(null);
    setSaved(false);
    setAsking(false);
    setReport(null);
    setUndoAsk(false);
    void bridge.call<SandboxStatus>('sandboxGet', { workspaceId }).then((next) => { if (next) take(next); }, (reason: unknown) => setError(String(reason instanceof Error ? reason.message : reason)));
  }, [bridge, open, workspaceId]);

  if (!open) return null;
  const notice = status ? sandboxNotice(status) : null;
  const locked = !status || notice !== null;
  const set = <K extends keyof SandboxStatus>(key: K, value: SandboxStatus[K]) => { setForm((prev) => ({ ...prev, [key]: value })); setSaved(false); };

  const save = async (approve = false) => {
    if (!status || busy) return;
    setBusy(true);
    setError(null);
    setReport(null);
    try {
      const next = await bridge.call<SandboxStatus>('sandboxSave', {
        approve,
        workspaceId: status.workspaceId,
        enabled: Boolean(form.enabled) && !locked,
        limitNetwork: Boolean(form.limitNetwork),
        allowedHosts: linesOf(hosts),
        protectInstructions: Boolean(form.protectInstructions),
        readOnlyPaths: linesOf(paths),
        reviewer: Boolean(form.reviewer),
        reviewerAgent: form.reviewerAgent ?? 'claude',
        reviewerModel: (form.reviewerModel ?? '').trim(),
      });
      if (next?.needsApproval) {
        setStatus(next);
        setAsking(true);
        return;
      }
      if (next) take(next);
      setAsking(false);
      setReport(next?.report ?? null);
      setSaved(true);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const undoAll = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const done = await bridge.call<SandboxReport>('sandboxUndoAll', {});
      setReport(done ?? null);
      setUndoAsk(false);
      const next = await bridge.call<SandboxStatus>('sandboxGet', { workspaceId: status?.workspaceId ?? workspaceId });
      if (next) take(next);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const windows = status?.windows ?? null;
  const changeList = windows && (
    <ul className="sandbox-changes" data-testid="sandbox-changes">
      {groupChanges(windows.changes).map((group) => (
        <li key={group.kind}>
          <strong>{t(keyOf(KIND_KEYS, group.kind))}</strong>
          <ul>
            {group.items.map((item) => (
              <li key={item.path}>
                <span className="mono">{item.path}</span>{' '}
                <span className="sandbox-help">{item.ops.map((op) => t(keyOf(OP_KEYS, op))).join(', ')}{item.made ? ' · ' + t('sandbox.windows.made') : ''}</span>
              </li>
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );

  const check = (key: 'enabled' | 'limitNetwork' | 'protectInstructions' | 'reviewer', label: string, help?: string, disabled = false) => (
    <label className="sandbox-check">
      <input type="checkbox" checked={Boolean(form[key])} disabled={disabled || busy} data-testid={'sandbox-' + key}
        onChange={(event) => set(key, event.target.checked)} />
      <span className="sandbox-check-words">
        <span>{label}</span>
        {help && <span className="sandbox-help">{help}</span>}
      </span>
    </label>
  );

  const statusLine = error ? <span className="tone-text-err">{error}</span> : saved ? <span>{t('sandbox.settings.saved')}</span> : undefined;
  return portal(
    <DialogShell
      kind="panel"
      panel="sandbox"
      width={560}
      height={620}
      minWidth={440}
      minHeight={360}
      label={t('sandbox.settings.title')}
      title={t('sandbox.settings.title')}
      subtitle={status?.name}
      icon={<Icon name="shield" size={18} weight={1.8} />}
      busy={busy}
      onCancel={onClose}
      closeTestId="sandbox-close"
      bodyClassName="sandbox-body"
      backdrop={{ 'data-page-overlay': '', 'data-testid': 'sandbox-panel' }}
      status={statusLine}
      buttons={(
        <>
          <button type="button" className="btn" onClick={onClose} disabled={busy}>{t('sandbox.settings.close')}</button>
          <button type="button" className="btn primary" data-testid="sandbox-save" onClick={() => void save()} disabled={busy || !status || asking}>{t('sandbox.settings.save')}</button>
        </>
      )}
    >
      {!status ? <DialogLoading rows={3} /> : (
        <div className="sandbox-form">
          <p className="sandbox-intro">{t('sandbox.settings.intro')}</p>
          {status.experimental && (
            <p className="sandbox-notice" data-testid="sandbox-experimental">
              <strong>{t('sandbox.settings.experimental')}</strong> {t('sandbox.settings.experimentalHelp')}
            </p>
          )}
          {notice && <p className="sandbox-notice" data-testid="sandbox-notice">{t(notice)}</p>}
          {status.guarantees && status.experimental && (
            <section className="sandbox-guarantees" data-testid="sandbox-guarantees">
              <strong>{t('sandbox.settings.guarantees')}</strong>
              <ul>
                {status.guarantees.map((g) => (
                  <li key={g.key}>
                    <span className={'sandbox-level sandbox-level-' + levelTone(g.level)}>{t(keyOf(LEVEL_KEYS, g.level))}</span>
                    <span>{t(keyOf(GUARANTEE_KEYS, g.key))}</span>
                  </li>
                ))}
              </ul>
            </section>
          )}
          {asking && windows && (
            <section className="sandbox-approval" data-testid="sandbox-approval">
              <strong>{t('sandbox.windows.approveTitle')}</strong>
              <span className="sandbox-help">{t('sandbox.windows.approveHelp')}</span>
              {changeList}
              <div className="sandbox-row">
                <button type="button" className="btn" onClick={() => setAsking(false)} disabled={busy}>{t('sandbox.windows.cancel')}</button>
                <button type="button" className="btn primary" data-testid="sandbox-approve" onClick={() => void save(true)} disabled={busy}>{t('sandbox.windows.approve')}</button>
              </div>
            </section>
          )}
          {check('enabled', t('sandbox.settings.enabled'), t('sandbox.settings.enabledHelp'), locked)}
          <fieldset className="sandbox-group" disabled={locked || busy}>
            {check('limitNetwork', t('sandbox.settings.network'), t(status.experimental ? 'sandbox.settings.networkHelpWindows' : 'sandbox.settings.networkHelp'))}
            <label className="sandbox-field">
              <span>{t('sandbox.settings.hosts')}</span>
              <textarea className="dialog-field sandbox-lines mono" rows={4} value={hosts} data-testid="sandbox-hosts" spellCheck={false}
                disabled={!form.limitNetwork} onChange={(event) => { setHosts(event.target.value); setSaved(false); }} />
              <span className="sandbox-help">{t('sandbox.settings.hostsHelp')}</span>
            </label>
            {check('protectInstructions', t('sandbox.settings.instructions'))}
            <label className="sandbox-field">
              <span>{t('sandbox.settings.readOnly')}</span>
              <textarea className="dialog-field sandbox-lines mono" rows={3} value={paths} data-testid="sandbox-paths" spellCheck={false}
                onChange={(event) => { setPaths(event.target.value); setSaved(false); }} />
              <span className="sandbox-help">{t('sandbox.settings.readOnlyHelp')}</span>
            </label>
            {check('reviewer', t('sandbox.settings.reviewer'), t('sandbox.settings.reviewerHelp'))}
            <div className="sandbox-row">
              <label className="sandbox-field">
                <span>{t('sandbox.settings.reviewerAgent')}</span>
                <select className="dialog-field" value={form.reviewerAgent ?? 'claude'} disabled={!form.reviewer} data-testid="sandbox-reviewer-agent"
                  onChange={(event) => set('reviewerAgent', event.target.value)}>
                  {status.reviewerAgents.map((agent) => <option key={agent} value={agent}>{providerLabel(agent)}</option>)}
                </select>
              </label>
              <label className="sandbox-field sandbox-grow">
                <span>{t('sandbox.settings.reviewerModel')}</span>
                <input className="dialog-field mono" value={form.reviewerModel ?? ''} disabled={!form.reviewer} placeholder={t('sandbox.settings.reviewerModelHint')}
                  data-testid="sandbox-reviewer-model" onChange={(event) => set('reviewerModel', event.target.value)} />
              </label>
            </div>
          </fieldset>
          {windows && !asking && (
            <section className="sandbox-blocked" data-testid="sandbox-windows">
              <strong>{t('sandbox.windows.changesTitle')}</strong>
              <span className="sandbox-help">
                {windows.approved ? t('sandbox.windows.approvedAt', { at: windows.approvedAt ?? '' }) : t('sandbox.windows.notYet')}
                {windows.approved && pendingChanges(windows) > 0 ? ' ' + t('sandbox.windows.pending', { count: String(pendingChanges(windows)) }) : ''}
              </span>
              {windows.error && <span className="tone-text-err">{windows.error}</span>}
              {changeList}
              {windows.recorded > 0 && (undoAsk ? (
                <div className="sandbox-row">
                  <span className="sandbox-help sandbox-grow">{t('sandbox.windows.undoAllAsk')}</span>
                  <button type="button" className="btn" onClick={() => setUndoAsk(false)} disabled={busy}>{t('sandbox.windows.cancel')}</button>
                  <button type="button" className="btn" data-testid="sandbox-undo-all-yes" onClick={() => void undoAll()} disabled={busy}>{t('sandbox.windows.undoAll')}</button>
                </div>
              ) : (
                <div className="sandbox-row">
                  <span className="sandbox-help sandbox-grow">{t('sandbox.windows.undoAllHelp')}</span>
                  <button type="button" className="btn" data-testid="sandbox-undo-all" onClick={() => setUndoAsk(true)} disabled={busy}>{t('sandbox.windows.undoAll')}</button>
                </div>
              ))}
            </section>
          )}
          {report && (report.applied > 0 || report.undone > 0 || report.failures.length > 0) && (
            <p className="sandbox-help" data-testid="sandbox-report">
              {t('sandbox.windows.report', { applied: String(report.applied), undone: String(report.undone), failed: String(report.failures.length) })}
              {report.failures.slice(0, 3).map((f) => ' ' + f.path + ': ' + f.reason).join(';')}
            </p>
          )}
          <section className="sandbox-blocked" data-testid="sandbox-blocked">
            <strong>{t('sandbox.settings.blocked')}</strong>
            {status.blocked.length === 0 ? <span className="sandbox-help">{t('sandbox.settings.noBlocked')}</span> : (
              <ul>
                {status.blocked.map((block, index) => (
                  <li key={index}><span className="mono">{block.what}</span> <span className="sandbox-help">{block.reason}</span></li>
                ))}
              </ul>
            )}
          </section>
        </div>
      )}
    </DialogShell>,
  );
}

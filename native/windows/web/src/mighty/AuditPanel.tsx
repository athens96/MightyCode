import { useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogShell } from '../dialogs/DialogShell';
import { Icon, type IconName } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import { AuditKinds, KindKeys, clock, rowMeta, summaryLine, type AuditCopied, type AuditKind, type AuditRow, type TurnAudit } from './audit';
import './audit.css';

export interface AuditPanelProps {
  audit: TurnAudit;
  onCopy(): Promise<AuditCopied>;
  onClose(): void;
}

const kindIcon: Record<AuditKind, IconName> = {
  request: 'message-square', command: 'square-terminal', edit: 'pencil-line', file: 'file-text', web: 'globe', tool: 'wrench',
  agent: 'users', child: 'square-stack', approval: 'shield', verify: 'badge-check', result: 'circle-check',
};

const shown = (text: string) => (text.length > 400 ? text.slice(0, 400) + '…' : text);

/**
 * MC-54: 감사 기록: one request's audit (audit.ts), one line a row in time order — its time, its kind, what it was (a command, a path,
 * an address in mono) and its state, tool, time taken and who in the request did it. The kinds it holds filter the list; the footer
 * says the counts and copies the audit as Markdown (the host takes secrets and the home folder out on the way to the clipboard).
 */
export function AuditPanel(props: AuditPanelProps) {
  return createPortal(<AuditDialog {...props} />, document.body);
}

export function AuditDialog({ audit, onCopy, onClose }: AuditPanelProps) {
  const { t } = useServices();
  usePageOverlay(true);
  const language = globalThis.document?.documentElement.lang || 'en';
  const [filter, setFilter] = useState<AuditKind | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  const kinds = useMemo(() => AuditKinds.filter((kind) => kind !== 'request' && kind !== 'result' && audit.rows.some((row) => row.kind === kind)), [audit]);
  const rows = filter ? audit.rows.filter((row) => row.kind === filter) : audit.rows;
  const empty = audit.rows.every((row) => row.kind === 'request' || row.kind === 'result');
  const copy = () => {
    setCopying(true);
    void onCopy().then((outcome) => {
      setCopied('error' in outcome ? t('turnAudit.copyFailed', { reason: outcome.error })
        : outcome.redacted > 0 ? t('turnAudit.copiedRedacted', { count: outcome.redacted }) : t('turnAudit.copied'));
    }).finally(() => setCopying(false));
  };
  const stop = (event: { stopPropagation(): void }) => event.stopPropagation();
  const status = copied ?? summaryLine(t, audit) + (audit.reads > 0 ? ' · ' + t('turnAudit.reads', { count: audit.reads }) : '');
  return (
    <DialogShell kind="panel" panel="turnAudit" width={760} height={560} minWidth={480} minHeight={320} label={t('turnAudit.title')}
      testId="turn-audit" icon={<Icon name="scroll-text" size={18} weight={1.8} />} title={t('turnAudit.title') + ' · ' + audit.title}
      subtitle={audit.request || undefined} closeTestId="turn-audit-close" onCancel={onClose} bodyClassName="turn-audit-body"
      backdrop={{ 'data-page-overlay': '', onPointerDown: stop, onWheel: stop }}
      status={<span data-testid="turn-audit-status">{status}</span>}
      buttons={<button type="button" className="btn primary" data-testid="turn-audit-copy" disabled={copying} title={t('turnAudit.copyMarkdown')} onClick={copy}>{t('turnAudit.copy')}</button>}>
      <p className="turn-audit-intro">{t('turnAudit.intro')}</p>
      {kinds.length > 1 && (
        <div className="turn-audit-filters" role="group" aria-label={t('turnAudit.filter.label')}>
          <button type="button" className={'turn-audit-filter' + (filter === null ? ' on' : '')} aria-pressed={filter === null} onClick={() => setFilter(null)}>{t('turnAudit.filter.all')}</button>
          {kinds.map((kind) => (
            <button key={kind} type="button" className={'turn-audit-filter' + (filter === kind ? ' on' : '')} aria-pressed={filter === kind} data-testid={'turn-audit-filter-' + kind}
              onClick={() => setFilter(filter === kind ? null : kind)}>
              {t(KindKeys[kind])} <span className="turn-audit-count">{audit.rows.filter((row) => row.kind === kind).length}</span>
            </button>
          ))}
        </div>
      )}
      {empty && <div className="turn-audit-empty" data-testid="turn-audit-empty">{t('turnAudit.empty')}</div>}
      <ol className="turn-audit-rows" data-testid="turn-audit-rows">
        {rows.map((row) => <AuditLine key={row.id} row={row} language={language} />)}
      </ol>
    </DialogShell>
  );
}

function AuditLine({ row, language }: { row: AuditRow; language: string }) {
  const { t } = useServices();
  const meta = rowMeta(t, row);
  return (
    <li className={'turn-audit-row kind-' + row.kind + ' state-' + row.state} data-testid={'turn-audit-row-' + row.id} data-kind={row.kind}>
      <span className="turn-audit-time">{clock(row.at, language)}</span>
      <span className="turn-audit-kind"><Icon name={kindIcon[row.kind]} size={12} weight={2} />{t(KindKeys[row.kind])}</span>
      <span className="turn-audit-what">
        <span className={row.code ? 'turn-audit-text code' : 'turn-audit-text'} title={row.text}>{shown(row.text)}</span>
        {meta.length > 0 && <span className="turn-audit-meta">{meta.join(' · ')}</span>}
      </span>
    </li>
  );
}

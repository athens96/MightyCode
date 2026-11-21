import { useLayoutEffect, useRef, type KeyboardEvent, type MouseEvent } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { Markdown } from '../conversation/MarkdownView';
import { useReferenceClick, useReferenceKey } from '../conversation/referenceClick';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import { DialogShell } from '../dialogs/DialogShell';
import type { PlanRecord } from './types';

/** One answered plan's meaning, kept with its text in both the history strip and the diagram. */
export function PlanRecordDetails({ record, expanded, metadata = true }: { record: PlanRecord; expanded: boolean; metadata?: boolean }) {
  const { t } = useServices();
  const onReferenceClick = useReferenceClick();
  const onReferenceKey = useReferenceKey();
  return <>
    {metadata && <div className="plan-record-meta">
      <span className={'plan-pill tone-' + record.tone}>{record.outcomeTitle}</span>
      <span className="plan-record-time">{record.time}</span>
    </div>}
    {expanded ? <div className="plan-document" onClick={onReferenceClick} onKeyDown={onReferenceKey}><Markdown text={record.plan} /></div> : metadata && <div className="plan-record-headline">{record.headline}</div>}
    {expanded && record.truncated && <div className="card-note">{t('plan.history.truncated')}</div>}
    {record.feedback && <div className={'plan-feedback' + (expanded ? ' open' : '')} title={record.feedback}>{t('plan.history.feedback')} · {record.feedback}</div>}
  </>;
}

export interface PlanDocumentProps {
  title: string;
  subtitle?: string;
  plan: string;
  onClose(): void;
}

/**
 * A pending or answered plan opened over the whole window: its title, when it was received and the plan text only
 * (M/PlanApprovalCard.swift PlanDocumentSheet), outside the graph's transformed/clipped layer: the popup standard's panel
 * (kind C, 860 × 677, its size kept as "plan"), ✕ in the header and no footer. Like the picture viewer it hides overlapping
 * native views (the Mac's Ghostty terminal), owns Escape/Tab, and returns focus to its Expand button on close.
 */
export function PlanDocument(props: PlanDocumentProps) {
  return createPortal(<PlanDocumentDialog {...props} />, document.body);
}

/** Exported separately so the document's content and metadata can be checked without a browser. */
export function PlanDocumentDialog({ title, subtitle, plan, onClose }: PlanDocumentProps) {
  const { t } = useServices();
  const card = useRef<HTMLDivElement | null>(null);
  const onReferenceClick = useReferenceClick();
  const onReferenceKey = useReferenceKey();
  usePageOverlay(true);
  useLayoutEffect(() => {
    card.current?.querySelector<HTMLElement>('[data-testid=plan-document-close]')?.focus({ preventScroll: true });
  }, []);
  const key = (event: KeyboardEvent) => {
    onReferenceKey(event);
    event.stopPropagation();
  };
  const stop = (event: { stopPropagation(): void }) => event.stopPropagation();
  return (
    <DialogShell kind="panel" panel="plan" width={860} height={677} minWidth={480} minHeight={320} label={title} testId="plan-document" frameRef={card}
      icon={<Icon name="clipboard-list" size={18} weight={1.8} />} title={title} subtitle={subtitle && <span className="plan-record-time">{subtitle}</span>}
      closeTestId="plan-document-close" onCancel={onClose} bodyClassName="card-dialog-document" onKeyDown={key} focus="none"
      backdrop={{
        'data-page-overlay': '', 'data-close-label': t('plan.card.close'), onPointerDown: stop, onWheel: stop,
        onClick: (event: MouseEvent<HTMLDivElement>) => { event.stopPropagation(); if (event.target === event.currentTarget) onClose(); },
      }}>
      <div onClick={onReferenceClick}>
        <Markdown text={plan} />
      </div>
    </DialogShell>
  );
}

// MC-73: the report box (B): a panel at the window's right edge with every bot's reports — filtered by status and bot, by day, what
// needs the person and what failed first, a day's ✅ folded into one line — each a card with its summary, items and links, cost and
// time, and what can be done about it: open the bot's pane, ask it more, run it again, 👍/👎. Opened from the 봇 tab (its 리포트
// button, a row's report line); the host keeps the reports (bot_report.rs).
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from './overlay';
import { BetaBadge } from './parts';
import {
  clockOf, filterReports, groupByDay, lengthWords, statusCounts, statusMarks,
  type BotReport, type ReportFilter, type ReportStatus,
} from './reportRules';
import './reportBox.css';

/** The event that opens the box (its detail a bot's handle to show alone, or none). */
export const openReportBoxEvent = 'mighty:open-report-box';
export function openReportBox(handle?: string) {
  window.dispatchEvent(new CustomEvent<string | undefined>(openReportBoxEvent, { detail: handle }));
}

const statusKeys: Record<ReportStatus, string> = {
  ok: 'bot.report.status.ok',
  report: 'bot.report.status.report',
  needs: 'bot.report.status.needs',
  failed: 'bot.report.status.failed',
};
const filterOrder: ReportFilter[] = ['all', 'needs', 'failed', 'report', 'ok'];

export interface ReportCardProps {
  report: BotReport;
  expanded: boolean;
  onToggle: () => void;
  onOpenPane: (focus: boolean) => void;
  onRerun: () => void;
  onThumb: (up: boolean) => void;
  onOpenUrl: (url: string) => void;
}

export function ReportCard({ report: r, expanded, onToggle, onOpenPane, onRerun, onThumb, onOpenUrl }: ReportCardProps) {
  const { t } = useServices();
  return (
    <article className={'report-card status-' + r.status + (r.read ? '' : ' unread')} data-testid="report-card">
      <button type="button" className="report-card-head" aria-expanded={expanded} onClick={onToggle}>
        {!r.read && <span className="report-card-unread" title={t('bot.report.unread')} aria-label={t('bot.report.unread')} data-testid="report-card-unread" />}
        <b>@{r.handle}</b>
        <span className="report-card-job">{[r.job, clockOf(r.at)].filter(Boolean).join(' · ')}</span>
        <span className="spacer" />
        {r.repeat > 1 && <span className="report-repeat">{t('bot.report.repeat', { count: r.repeat })}</span>}
        <span className={'report-status status-' + r.status}>{statusMarks[r.status]} {t(statusKeys[r.status])}</span>
      </button>
      <p className="report-summary">{r.summary}</p>
      {r.items.length > 0 && (
        <ul className="report-items">
          {r.items.map((item, i) => (
            <li key={i}>
              {item.url ? <button type="button" className="link-button" title={item.url} onClick={() => onOpenUrl(item.url!)}>{item.text}</button> : item.text}
            </li>
          ))}
        </ul>
      )}
      {expanded && (
        <div className="report-meta">
          {r.reason && <span>{t('bot.report.trigger', { reason: r.reason })}</span>}
          {typeof r.seconds === 'number' && <span>⏱ {lengthWords(r.seconds, (n) => t('bot.report.minutes', { n }), (n) => t('bot.report.seconds', { n }))}</span>}
          {typeof r.costUsd === 'number' && <span>💲{r.costUsd.toFixed(2)}</span>}
          {r.eventUrl && <button type="button" className="link-button" onClick={() => onOpenUrl(r.eventUrl!)}>{t('bot.report.event')}</button>}
        </div>
      )}
      <div className="report-actions">
        {r.pane && <button type="button" className={'btn' + (r.status === 'needs' ? ' primary' : '')} onClick={() => onOpenPane(r.status === 'needs')}>{t(r.status === 'needs' ? 'bot.report.answer' : 'bot.report.open')}</button>}
        {r.pane && r.status !== 'needs' && <button type="button" className="btn" onClick={() => onOpenPane(true)}>{t('bot.report.followUp')}</button>}
        {r.record && <button type="button" className={'btn' + (r.status === 'failed' ? ' primary' : '')} onClick={onRerun}>{t('bot.report.rerun')}</button>}
        <span className="spacer" />
        <button type="button" className={'report-thumb' + (r.thumb === true ? ' on' : '')} aria-pressed={r.thumb === true} title={t('bot.report.useful')} onClick={() => onThumb(true)}>👍</button>
        <button type="button" className={'report-thumb' + (r.thumb === false ? ' on' : '')} aria-pressed={r.thumb === false} title={t('bot.report.notUseful')} onClick={() => onThumb(false)}>👎</button>
      </div>
    </article>
  );
}

/** Mounted once in the shell: shows the box while it is open. */
export function ReportBoxHost() {
  const [open, setOpen] = useState<{ handle: string | null } | null>(null);
  useEffect(() => {
    const show = (event: Event) => setOpen({ handle: (event as CustomEvent<string | undefined>).detail ?? null });
    window.addEventListener(openReportBoxEvent, show);
    return () => window.removeEventListener(openReportBoxEvent, show);
  }, []);
  return open ? <ReportBox handle={open.handle} onClose={() => setOpen(null)} /> : null;
}

function ReportBox({ handle: initial, onClose }: { handle: string | null; onClose: () => void }) {
  const { t, bridge, store } = useServices();
  usePageOverlay(true);
  const [reports, setReports] = useState<BotReport[] | null>(null);
  const [filter, setFilter] = useState<ReportFilter>('all');
  const [handle, setHandle] = useState<string | null>(initial);
  const [unfolded, setUnfolded] = useState<string[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  // Read again whenever the host's count of unread reports moves (a new one came, one was read).
  const unread = useStoreSelector((state) => state.shell?.botReportsUnread ?? 0);
  const load = useCallback(() => {
    void bridge.call<{ reports: BotReport[] }>('botReports').then((value) => setReports(value.reports), () => setReports([]));
  }, [bridge]);
  useEffect(load, [load, unread]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [onClose]);

  const handles = useMemo(() => [...new Set((reports ?? []).map((r) => r.handle))].sort(), [reports]);
  const counts = useMemo(() => statusCounts(filterReports(reports ?? [], 'all', handle)), [reports, handle]);
  const days = useMemo(() => groupByDay(filterReports(reports ?? [], filter, handle), new Date(), filter === 'ok'), [reports, filter, handle]);

  const read = (id?: string) => {
    setReports((list) => list?.map((r) => (id === undefined || r.id === id ? { ...r, read: true } : r)) ?? null);
    void bridge.call('botReportRead', id ? { id } : {}).catch(() => {});
  };
  const openPane = (report: BotReport, focus: boolean) => {
    read(report.id);
    if (!report.pane) return;
    store.dispatch({ kind: 'choose', paneId: report.pane });
    void bridge.call('selectSession', { id: report.pane }).catch(() => {});
    if (focus) window.setTimeout(() => (document.querySelector('[data-testid="composer-input"]') as HTMLTextAreaElement | null)?.focus(), 150);
  };
  const rerun = (report: BotReport) => {
    read(report.id);
    void bridge.call<{ ok?: boolean; error?: string }>('botReportRerun', { id: report.id }).then(
      (answer) => setNote(answer.error ?? t('bot.report.rerunStarted', { handle: report.handle })),
      () => setNote(t('automation.retry.gone')),
    );
  };
  const thumb = (report: BotReport, up: boolean) => {
    const next = report.thumb === up ? null : up;
    setReports((list) => list?.map((r) => (r.id === report.id ? { ...r, thumb: next } : r)) ?? null);
    void bridge.call('botReportThumb', { id: report.id, up: next }).catch(() => {});
  };
  const dayTitle = (key: string) => (key === 'today' ? t('bot.report.today') : key === 'yesterday' ? t('bot.report.yesterday') : key);

  const openUrl = (url: string) => void bridge.call('openUrl', { url }).catch(() => {});
  const card = (r: BotReport) => {
    const expanded = open === r.id;
    return (
      <ReportCard
        key={r.id}
        report={r}
        expanded={expanded}
        onToggle={() => { setOpen(expanded ? null : r.id); if (!r.read) read(r.id); }}
        onOpenPane={(focus) => openPane(r, focus)}
        onRerun={() => rerun(r)}
        onThumb={(up) => thumb(r, up)}
        onOpenUrl={openUrl}
      />
    );
  };

  return createPortal(
    <aside className="report-box" data-page-overlay="" data-testid="report-box" role="dialog" aria-label={t('bot.report.title')}>
      <header className="report-box-head">
        <Icon name="list-checks" size={15} />
        <b>{t('bot.report.title')}</b>
        <BetaBadge />
        {unread > 0 && <span className="report-box-count">{unread}</span>}
        <span className="spacer" />
        <button type="button" className="btn text" disabled={unread === 0} onClick={() => read()}>{t('bot.report.readAll')}</button>
        <button type="button" className="icon-button" title={t('bot.dialog.cancel')} aria-label={t('bot.dialog.cancel')} onClick={onClose}><Icon name="x" size={14} /></button>
      </header>
      <div className="report-box-filters">
        {filterOrder.map((f) => (
          <button key={f} type="button" className={'bot-wizard-chip' + (filter === f ? ' on' : '')} aria-pressed={filter === f} onClick={() => setFilter(f)}>
            {f === 'all' ? t('bot.report.all') : `${statusMarks[f]} ${t(statusKeys[f])} ${counts[f]}`}
          </button>
        ))}
        <select className="dialog-field report-box-bot" value={handle ?? ''} aria-label={t('bot.report.bot')} onChange={(e) => setHandle(e.target.value || null)}>
          <option value="">{t('bot.report.allBots')}</option>
          {handles.map((h) => <option key={h} value={h}>@{h}</option>)}
        </select>
      </div>
      {note && <div className="report-box-note" role="status">{note}</div>}
      <div className="report-box-body">
        {reports === null ? <p className="bot-wizard-muted">…</p> : days.length === 0 ? <p className="bot-wizard-muted">{t('bot.report.empty')}</p> : days.map((day) => (
          <section key={day.key}>
            <h4 className="report-day">{dayTitle(day.key)}</h4>
            {day.shown.map(card)}
            {day.quiet.length > 0 && (unfolded.includes(day.key) ? day.quiet.map(card) : (
              <button type="button" className="report-fold" onClick={() => setUnfolded([...unfolded, day.key])}>
                ✅ {t('bot.report.quiet', { count: day.quiet.length, bots: [...new Set(day.quiet.map((r) => '@' + r.handle))].join(' ') })}
              </button>
            ))}
          </section>
        ))}
      </div>
    </aside>,
    document.body,
  );
}

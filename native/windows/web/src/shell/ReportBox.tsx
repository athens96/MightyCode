// MC-90: the 알림 함 (once MC-73's report box): a panel at the window's right edge with two tabs. 일반 알림 holds a workspace's
// finished work (✅ done, ❌ failed: its pane, the request and a one-line summary, the host's notices.rs); 봇 알림 holds the bots'
// reports (MC-73: filtered by status and bot, by day, what needs the person and what failed first, a day's ✅ folded into one line,
// each with its items and links, cost and time, open the bot's pane, ask it more, run it again, 👍/👎; bot_report.rs). A card's
// 이동, or a double click on it, opens its pane and the pane's diagram on that run's request block. Opened from the status bar's bell
// (shell/Chrome.tsx), the 봇 tab's 리포트 and a bot row's menu (the 봇 알림 tab, that bot alone). MC-97: a third tab, 이벤트, lists the
// events that reached the automation layer and what each trigger listening to them did (eventRules.ts), with 시험 실행 per trigger.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from './overlay';
import { BetaBadge } from './parts';
import {
  eventDays, eventDots, eventTone, eventToneKeys, outcomeWords, shownFields, triggerRows, type EventRecord,
} from './eventRules';
import {
  clockOf, filterReports, groupByDay, lengthWords, noticeCounts, noticeDays, noticeMarks, openingTab, statusCounts, statusMarks,
  type BotReport, type Notice, type NoticeFilter, type NoticeStatus, type NoticeTab, type ReportFilter, type ReportStatus,
} from './reportRules';
import './reportBox.css';

/** The event that opens the box: its detail the tab and, for 봇 알림, a bot's handle to show alone. */
export const openReportBoxEvent = 'mighty:open-report-box';
interface OpenDetail { tab?: NoticeTab; handle?: string }
/** The 봇 알림 tab (a bot's own reports when `handle` is given). */
export function openReportBox(handle?: string) {
  window.dispatchEvent(new CustomEvent<OpenDetail>(openReportBoxEvent, { detail: { tab: 'bots', handle } }));
}
/** The bell: the tab with unread notices first. */
export function openNotices() {
  window.dispatchEvent(new CustomEvent<OpenDetail>(openReportBoxEvent, { detail: {} }));
}

const statusKeys: Record<ReportStatus, string> = {
  ok: 'bot.report.status.ok',
  report: 'bot.report.status.report',
  needs: 'bot.report.status.needs',
  failed: 'bot.report.status.failed',
};
const noticeKeys: Record<NoticeStatus, string> = { ok: 'notice.status.ok', failed: 'notice.status.failed' };
const filterOrder: ReportFilter[] = ['all', 'needs', 'failed', 'report', 'ok'];
const noticeFilterOrder: NoticeFilter[] = ['all', 'failed', 'ok'];

export interface ReportCardProps {
  report: BotReport;
  expanded: boolean;
  onToggle: () => void;
  onOpenPane: (focus: boolean) => void;
  onGo: () => void;
  onRerun: () => void;
  onThumb: (up: boolean) => void;
  onOpenUrl: (url: string) => void;
}

export function ReportCard({ report: r, expanded, onToggle, onOpenPane, onGo, onRerun, onThumb, onOpenUrl }: ReportCardProps) {
  const { t } = useServices();
  return (
    <article className={'report-card status-' + r.status + (r.read ? '' : ' unread')} data-testid="report-card"
      title={r.pane ? t('notice.goHelp') : undefined} onDoubleClick={r.pane ? onGo : undefined}>
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
        {r.pane && <button type="button" className="btn" data-testid="report-card-go" title={t('notice.goHelp')} onClick={onGo}>{t('notice.go')}</button>}
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

/** MC-90: a general notice: where it ran, when, how it ended, what it was asked and what it said; 이동 (or a double click) opens it. */
export function NoticeCard({ notice: n, onRead, onGo, onClear }: { notice: Notice; onRead: () => void; onGo: () => void; onClear: () => void }) {
  const { t } = useServices();
  return (
    <article className={'report-card notice-card status-' + n.status + (n.read ? '' : ' unread')} data-testid="notice-card"
      title={t('notice.goHelp')} onDoubleClick={onGo}>
      <button type="button" className="report-card-head" onClick={onRead}>
        {!n.read && <span className="report-card-unread" title={t('bot.report.unread')} aria-label={t('bot.report.unread')} />}
        <b>{n.title}</b>
        <span className="report-card-job">
          {[n.workspace, clockOf(n.at), typeof n.seconds === 'number' ? '⏱ ' + lengthWords(n.seconds, (m) => t('bot.report.minutes', { n: m }), (s) => t('bot.report.seconds', { n: s })) : '']
            .filter(Boolean).join(' · ')}
        </span>
        <span className="spacer" />
        <span className={'report-status status-' + n.status}>{noticeMarks[n.status]} {t(noticeKeys[n.status])}</span>
      </button>
      <p className="report-summary">{n.summary}</p>
      {n.request && <p className="notice-request"><span>{t('notice.request')}</span>{n.request}</p>}
      <div className="report-actions">
        <button type="button" className={'btn' + (n.status === 'failed' ? ' primary' : '')} data-testid="notice-card-go" onClick={onGo}>{t('notice.go')}</button>
        <span className="spacer" />
        <button type="button" className="icon-button" title={t('notice.clear')} aria-label={t('notice.clear')} onClick={onClear}><Icon name="x" size={12} /></button>
      </div>
    </article>
  );
}

/** MC-97: one event of the 이벤트 tab: its type and subject, where it came from and when, its title; unfolded, each trigger it reached
 *  with ✓ fired or ✗ why, 시험 실행 for each, and the fields a `where` condition may read. */
export function EventCard({ record: r, expanded, onToggle, onReplay, onOpenUrl }: {
  record: EventRecord; expanded: boolean; onToggle: () => void; onReplay: (trigger: string) => void; onOpenUrl: (url: string) => void;
}) {
  const { t } = useServices();
  const e = r.event;
  const tone = eventTone(r);
  const rows = triggerRows(r);
  const fields = shownFields(e);
  return (
    <article className="report-card event-card" data-testid="event-card">
      <button type="button" className="report-card-head" aria-expanded={expanded} onClick={onToggle}>
        <span className={'tone-dot ' + eventDots[tone]} aria-hidden="true" />
        <b className="event-type">{e.type || '—'}</b>
        <span className="report-card-job">{e.subject}</span>
        <span className="spacer" />
        <span className={'report-status event-status tone-' + tone}>{t(eventToneKeys[tone])}</span>
      </button>
      <div className="event-meta">{[e.source, clockOf(e.time), e.actor ?? ''].filter(Boolean).join(' · ')}</div>
      {e.title && <p className="report-summary">{e.title}</p>}
      {expanded && (
        <div className="event-detail">
          {rows.length === 0 && <p className="bot-wizard-muted">{t('automation.events.noTrigger')}</p>}
          {rows.map((row) => (
            <div key={row.key} className="event-trigger" data-testid="event-trigger">
              <div className="event-trigger-head">
                <b>{row.name}</b>
                {row.workspace && <span className="report-card-job">{row.workspace}</span>}
                <span className="spacer" />
                <button type="button" className="btn" data-testid="event-replay" title={t('automation.events.replayHelp')} onClick={() => onReplay(row.key)}>
                  {t('automation.events.replay')}
                </button>
              </div>
              <ul className="event-outcomes">
                {row.outcomes.map((o, i) => (
                  <li key={i} className={'outcome-' + o.outcome}>
                    {outcomeWords(o, t)}{o.replay ? ` · ${t('automation.events.replay')}` : ''}
                  </li>
                ))}
              </ul>
            </div>
          ))}
          {e.url && <button type="button" className="link-button" onClick={() => onOpenUrl(e.url!)}>{t('bot.report.event')}</button>}
          {fields.length > 0 && (
            <details className="event-fields">
              <summary>{t('automation.events.fields')}</summary>
              <dl>{fields.map(([key, value]) => <div key={key}><dt>{key}</dt><dd>{value}</dd></div>)}</dl>
            </details>
          )}
        </div>
      )}
    </article>
  );
}

/** Mounted once in the shell: shows the box while it is open. */
export function ReportBoxHost() {
  const [open, setOpen] = useState<OpenDetail | null>(null);
  useEffect(() => {
    const show = (event: Event) => setOpen((event as CustomEvent<OpenDetail | undefined>).detail ?? {});
    window.addEventListener(openReportBoxEvent, show);
    return () => window.removeEventListener(openReportBoxEvent, show);
  }, []);
  return open ? <ReportBox key={(open.tab ?? '') + (open.handle ?? '')} tab={open.tab} handle={open.handle ?? null} onClose={() => setOpen(null)} /> : null;
}

function ReportBox({ tab: asked, handle: initial, onClose }: { tab?: NoticeTab; handle: string | null; onClose: () => void }) {
  const { t, bridge, store } = useServices();
  usePageOverlay(true);
  const reportsUnread = useStoreSelector((state) => state.shell?.botReportsUnread ?? 0);
  const noticesUnread = useStoreSelector((state) => state.shell?.noticesUnread ?? 0);
  const eventsVersion = useStoreSelector((state) => state.shell?.eventInboxVersion ?? 0);
  const [tab, setTab] = useState<NoticeTab>(() => asked ?? openingTab(noticesUnread, reportsUnread));
  const [reports, setReports] = useState<BotReport[] | null>(null);
  const [notices, setNotices] = useState<Notice[] | null>(null);
  const [events, setEvents] = useState<EventRecord[] | null>(null);
  const [filter, setFilter] = useState<ReportFilter>('all');
  const [noticeFilter, setNoticeFilter] = useState<NoticeFilter>('all');
  const [handle, setHandle] = useState<string | null>(initial);
  const [unfolded, setUnfolded] = useState<string[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  // Read again whenever the host's unread counts move (a new one came, one was read).
  const loadReports = useCallback(() => {
    void bridge.call<{ reports: BotReport[] }>('botReports').then((value) => setReports(value.reports), () => setReports([]));
  }, [bridge]);
  const loadNotices = useCallback(() => {
    void bridge.call<{ notices: Notice[] }>('notices').then((value) => setNotices(value.notices), () => setNotices([]));
  }, [bridge]);
  useEffect(loadReports, [loadReports, reportsUnread]);
  useEffect(loadNotices, [loadNotices, noticesUnread]);
  // MC-97: the event inbox, read while its tab shows and again whenever it changes.
  const loadEvents = useCallback(() => {
    if (tab !== 'events') return;
    void bridge.call<{ events: EventRecord[] }>('automationEvents').then((value) => setEvents(value.events), () => setEvents([]));
  }, [bridge, tab]);
  useEffect(loadEvents, [loadEvents, eventsVersion]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [onClose]);

  const handles = useMemo(() => [...new Set((reports ?? []).map((r) => r.handle))].sort(), [reports]);
  const counts = useMemo(() => statusCounts(filterReports(reports ?? [], 'all', handle)), [reports, handle]);
  const days = useMemo(() => groupByDay(filterReports(reports ?? [], filter, handle), new Date(), filter === 'ok'), [reports, filter, handle]);
  const noticeCount = useMemo(() => noticeCounts(notices ?? []), [notices]);
  const noticeByDay = useMemo(() => noticeDays(notices ?? [], noticeFilter, new Date()), [notices, noticeFilter]);
  const eventsByDay = useMemo(() => eventDays(events ?? [], new Date()), [events]);

  // MC-91: what the open tab lists is read as soon as it shows (on opening, on a tab switch, and what comes in while it is open);
  // the ones new to this visit keep their dot until the box closes, so what is new still shows.
  const [seenNew, setSeenNew] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    if (tab !== 'general' || !notices) return;
    const fresh = notices.filter((n) => !n.read).map((n) => n.id);
    if (fresh.length === 0) return;
    setSeenNew((seen) => new Set([...seen, ...fresh]));
    setNotices((list) => list?.map((n) => (n.read ? n : { ...n, read: true })) ?? null);
    void bridge.call('noticeRead', {}).catch(() => {});
  }, [tab, notices, bridge]);
  useEffect(() => {
    if (tab !== 'bots' || !reports) return;
    const fresh = filterReports(reports, 'all', handle).filter((r) => !r.read).map((r) => r.id);
    if (fresh.length === 0) return;
    setSeenNew((seen) => new Set([...seen, ...fresh]));
    setReports((list) => list?.map((r) => (fresh.includes(r.id) ? { ...r, read: true } : r)) ?? null);
    for (const id of fresh) void bridge.call('botReportRead', { id }).catch(() => {});
  }, [tab, reports, handle, bridge]);
  const shownRead = <T extends { id: string; read: boolean }>(item: T): T => (seenNew.has(item.id) ? { ...item, read: false } : item);

  const read = (id?: string) => {
    setReports((list) => list?.map((r) => (id === undefined || r.id === id ? { ...r, read: true } : r)) ?? null);
    void bridge.call('botReportRead', id ? { id } : {}).catch(() => {});
  };
  const readNotice = (id?: string) => {
    setNotices((list) => list?.map((n) => (id === undefined || n.id === id ? { ...n, read: true } : n)) ?? null);
    void bridge.call('noticeRead', id ? { id } : {}).catch(() => {});
  };
  const clearNotice = (id?: string) => {
    setNotices((list) => list?.filter((n) => (id === undefined ? !n.read : n.id !== id)) ?? null);
    void bridge.call('noticeClear', id ? { id } : {}).catch(() => {});
  };
  const select = (pane: string) => {
    store.dispatch({ kind: 'choose', paneId: pane });
    void bridge.call('selectSession', { id: pane }).catch(() => {});
  };
  // 이동: the pane chosen (its workspace with it) and its diagram opened on the run's request block; the box makes way.
  const go = (pane: string | null | undefined, run: string | null | undefined) => {
    if (!pane) return;
    if (!store.getState().snapshot?.sessions.some((s) => s.id === pane)) {
      setNote(t('notice.paneGone'));
      return;
    }
    select(pane);
    store.dispatch({ kind: 'aim', paneId: pane, ...(run ? { runId: run } : {}) });
    onClose();
  };
  const openPane = (report: BotReport, focus: boolean) => {
    read(report.id);
    if (!report.pane) return;
    select(report.pane);
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
  // MC-97: 시험 실행: the event run again against one trigger; the host says what came of it.
  const replay = (record: EventRecord, trigger: string) => {
    void bridge.call<{ ok?: boolean; note?: string; error?: string }>('automationEventReplay', { id: record.id, trigger }).then(
      (answer) => setNote(answer.error ?? answer.note ?? null),
      () => setNote(t('automation.events.gone')),
    );
  };
  const clearEvents = () => {
    setEvents([]);
    void bridge.call('automationEventsClear', {}).catch(() => {});
  };
  const dayTitle = (key: string) => (key === 'today' ? t('bot.report.today') : key === 'yesterday' ? t('bot.report.yesterday') : key);

  const openUrl = (url: string) => void bridge.call('openUrl', { url }).catch(() => {});
  const card = (r: BotReport) => {
    const expanded = open === r.id;
    return (
      <ReportCard
        key={r.id}
        report={shownRead(r)}
        expanded={expanded}
        onToggle={() => { setOpen(expanded ? null : r.id); if (!r.read) read(r.id); }}
        onOpenPane={(focus) => openPane(r, focus)}
        onGo={() => { read(r.id); go(r.pane, r.run); }}
        onRerun={() => rerun(r)}
        onThumb={(up) => thumb(r, up)}
        onOpenUrl={openUrl}
      />
    );
  };
  const noticeCard = (n: Notice) => (
    <NoticeCard key={n.id} notice={shownRead(n)} onRead={() => { if (!n.read) readNotice(n.id); }} onGo={() => { readNotice(n.id); go(n.pane, n.run); }} onClear={() => clearNotice(n.id)} />
  );

  return createPortal(
    <aside className="report-box" data-page-overlay="" data-testid="report-box" role="dialog" aria-label={t('notice.title')}>
      <header className="report-box-head">
        <Icon name="bell" size={15} />
        <b>{t('notice.title')}</b>
        <span className="spacer" />
        {tab === 'general' && <button type="button" className="btn text" disabled={!(notices ?? []).some((n) => n.read)} onClick={() => clearNotice()}>{t('notice.clearRead')}</button>}
        {tab === 'events' && <button type="button" className="btn text" disabled={(events ?? []).length === 0} onClick={clearEvents}>{t('automation.events.clear')}</button>}
        <button type="button" className="icon-button" title={t('bot.dialog.cancel')} aria-label={t('bot.dialog.cancel')} onClick={onClose}><Icon name="x" size={14} /></button>
      </header>
      <div className="report-box-tabs" role="tablist" aria-label={t('notice.title')}>
        <button type="button" role="tab" aria-selected={tab === 'general'} className={tab === 'general' ? 'on' : ''} data-testid="notice-tab-general" onClick={() => setTab('general')}>
          {t('notice.tab.general')}{noticesUnread > 0 && <span className="report-box-count">{noticesUnread}</span>}
        </button>
        <button type="button" role="tab" aria-selected={tab === 'bots'} className={tab === 'bots' ? 'on' : ''} data-testid="notice-tab-bots" onClick={() => setTab('bots')}>
          {t('notice.tab.bots')}<BetaBadge />{reportsUnread > 0 && <span className="report-box-count">{reportsUnread}</span>}
        </button>
        <button type="button" role="tab" aria-selected={tab === 'events'} className={tab === 'events' ? 'on' : ''} data-testid="notice-tab-events" onClick={() => setTab('events')}>
          {t('notice.tab.events')}
        </button>
      </div>
      {tab === 'events' ? null : tab === 'general' ? (
        <div className="report-box-filters">
          {noticeFilterOrder.map((f) => (
            <button key={f} type="button" className={'bot-wizard-chip' + (noticeFilter === f ? ' on' : '')} aria-pressed={noticeFilter === f} onClick={() => setNoticeFilter(f)}>
              {f === 'all' ? t('bot.report.all') : `${noticeMarks[f]} ${t(noticeKeys[f])} ${noticeCount[f]}`}
            </button>
          ))}
        </div>
      ) : (
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
      )}
      {note && <div className="report-box-note" role="status">{note}</div>}
      <div className="report-box-body">
        {tab === 'events'
          ? (events === null ? <p className="bot-wizard-muted">…</p> : eventsByDay.length === 0 ? <p className="bot-wizard-muted">{t('automation.events.empty')}</p> : eventsByDay.map((day) => (
            <section key={day.key}>
              <h4 className="report-day">{dayTitle(day.key)}</h4>
              {day.events.map((r) => (
                <EventCard key={r.id} record={r} expanded={open === r.id} onToggle={() => setOpen(open === r.id ? null : r.id)}
                  onReplay={(trigger) => replay(r, trigger)} onOpenUrl={openUrl} />
              ))}
            </section>
          )))
          : tab === 'general'
          ? (notices === null ? <p className="bot-wizard-muted">…</p> : noticeByDay.length === 0 ? <p className="bot-wizard-muted">{t('notice.empty')}</p> : noticeByDay.map((day) => (
            <section key={day.key}>
              <h4 className="report-day">{dayTitle(day.key)}</h4>
              {day.notices.map(noticeCard)}
            </section>
          )))
          : (reports === null ? <p className="bot-wizard-muted">…</p> : days.length === 0 ? <p className="bot-wizard-muted">{t('bot.report.empty')}</p> : days.map((day) => (
            <section key={day.key}>
              <h4 className="report-day">{dayTitle(day.key)}</h4>
              {day.shown.map(card)}
              {day.quiet.length > 0 && (unfolded.includes(day.key) ? day.quiet.map(card) : (
                <button type="button" className="report-fold" onClick={() => setUnfolded([...unfolded, day.key])}>
                  ✅ {t('bot.report.quiet', { count: day.quiet.length, bots: [...new Set(day.quiet.map((r) => '@' + r.handle))].join(' ') })}
                </button>
              ))}
            </section>
          )))}
      </div>
    </aside>,
    document.body,
  );
}

// MC-107: the phone's 봇 tab rules, mirrored from the desktop's (native/windows/web/src/shell/botTab.ts `groupByKind`, `botState`,
// `statusLine`): every bot grouped by kind (독립 봇, then 워크스페이스 봇), each group in state order (needs you, working, idle,
// asleep) so a bot waiting for the person stands at the top, with one line saying what it is doing. Pure.
import type {
  BotReportStatus,
  BotState,
  MobileBot,
  MobileBotApproval,
  MobileBotReport,
  MobileBots,
} from '@/api/types';
import type { Tone } from '@/lib/status-tone';

export const BOT_STATES: readonly BotState[] = ['needsYou', 'working', 'idle', 'asleep'];
export type BotKind = MobileBot['kind'];

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

/** The state word under the desktop's own keys (`bot.sidebar.state.*`), literal so the locale checker sees each one. */
export const BOT_STATE_KEYS: Record<BotState, string> = {
  needsYou: 'bot.sidebar.state.needsYou',
  working: 'bot.sidebar.state.working',
  idle: 'bot.sidebar.state.idle',
  asleep: 'bot.sidebar.state.asleep',
};

/** A report's status word (`bot.report.status.*`). */
export const REPORT_STATUS_KEYS: Record<BotReportStatus, string> = {
  ok: 'bot.report.status.ok',
  report: 'bot.report.status.report',
  needs: 'bot.report.status.needs',
  failed: 'bot.report.status.failed',
};

/** The dot's tone for a bot's state: waiting amber, working blue, idle neutral, asleep grey. */
export function botTone(state: BotState): Tone {
  switch (state) {
    case 'needsYou':
      return 'wait';
    case 'working':
      return 'run';
    case 'asleep':
      return 'stop';
    case 'idle':
      return 'idle';
  }
}

/** The dot's tone for a report: nothing to do done-green, a report blue, needs amber, failed red. */
export function reportTone(status: BotReportStatus): Tone {
  switch (status) {
    case 'ok':
      return 'done';
    case 'needs':
      return 'wait';
    case 'failed':
      return 'err';
    case 'report':
      return 'run';
  }
}

/** The bots in state order, the order given kept within a state. */
export function inStateOrder(bots: readonly MobileBot[]): MobileBot[] {
  return BOT_STATES.flatMap((state) => bots.filter((bot) => bot.state === state));
}

/** The 봇 tab's grouping: the independent bots first, then the workspace bots, each in state order; empty groups left out. */
export function groupByKind(bots: readonly MobileBot[]): { kind: BotKind; bots: MobileBot[] }[] {
  return [
    { kind: 'independent' as const, bots: inStateOrder(bots.filter((bot) => bot.kind !== 'workspace')) },
    { kind: 'workspace' as const, bots: inStateOrder(bots.filter((bot) => bot.kind === 'workspace')) },
  ].filter((group) => group.bots.length > 0);
}

/**
 * The one line under a bot's name: why it waits, what it is on, that it sleeps, its next run (`at` already in words), else where it
 * works (its pane's title, or 독립).
 */
export function statusLine(bot: MobileBot, nextRun: string | undefined, t: Translate): string {
  switch (bot.state) {
    case 'needsYou':
      return bot.detail || t('bot.sidebar.line.needsYou');
    case 'working':
      return bot.detail || t('bot.sidebar.line.working');
    case 'asleep':
      return t('bot.sidebar.asleep');
    case 'idle':
      if (nextRun) return t('bot.sidebar.nextRun', { at: nextRun });
      return bot.kind === 'workspace' ? (bot.title ?? '') : t('bot.sidebar.independent');
  }
}

/** A time in words, as the desktop's 봇 tab says a next run: the clock today, else the day and the clock. */
export function timeWords(iso: string | undefined, now: Date, locale?: string): string | undefined {
  if (!iso) return undefined;
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return undefined;
  const clock = at.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', hour12: false });
  const sameDay = at.getFullYear() === now.getFullYear() && at.getMonth() === now.getMonth() && at.getDate() === now.getDate();
  if (sameDay) return clock;
  return `${at.toLocaleDateString(locale, { month: 'numeric', day: 'numeric' })} ${clock}`;
}

/** How many things wait for the person on one host: the approvals and the bots waiting. */
export function waitingCount(bots: MobileBots | undefined): number {
  if (!bots) return 0;
  return bots.approvals.length + bots.bots.filter((bot) => bot.state === 'needsYou').length;
}

/** A bot's reports, the newest first (the payload's order). */
export function reportsOf(reports: readonly MobileBotReport[], handle: string): MobileBotReport[] {
  const key = handleKey(handle);
  return reports.filter((report) => handleKey(report.handle) === key);
}

/** A handle as the desktop compares it: no leading @, composed (NFC), without case. */
export function handleKey(handle: string): string {
  return handle.trim().replace(/^@+/, '').normalize('NFC').toLowerCase();
}

/** A link a phone may open: http(s) only (the host already sends only those). */
export function openableLink(url: string | undefined): string | undefined {
  return url && /^https?:\/\//i.test(url) ? url : undefined;
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function list(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

const STATES = new Set<string>(BOT_STATES);
const STATUSES = new Set<string>(['ok', 'report', 'needs', 'failed']);

function status(value: unknown): BotReportStatus {
  return typeof value === 'string' && STATUSES.has(value) ? (value as BotReportStatus) : 'report';
}

function parseBot(value: unknown): MobileBot | undefined {
  const raw = record(value);
  const id = text(raw?.id);
  const handle = text(raw?.handle);
  if (!raw || !id || !handle) return undefined;
  const state = typeof raw.state === 'string' && STATES.has(raw.state) ? (raw.state as BotState) : 'idle';
  const report = record(raw.report);
  const bot: MobileBot = {
    id,
    handle,
    kind: raw.kind === 'workspace' ? 'workspace' : 'independent',
    state,
    asleep: raw.asleep === true,
    jobs: typeof raw.jobs === 'number' && Number.isFinite(raw.jobs) ? raw.jobs : 0,
    jobNames: list(raw.jobNames).filter((name): name is string => typeof name === 'string'),
  };
  const workspace = text(raw.workspace);
  if (workspace !== undefined) bot.workspace = workspace;
  const workspaceId = text(raw.workspaceId);
  if (workspaceId !== undefined) bot.workspaceId = workspaceId;
  const detail = text(raw.detail);
  if (detail) bot.detail = detail;
  const title = text(raw.title);
  if (title !== undefined) bot.title = title;
  const nextRun = text(raw.nextRun);
  if (nextRun) bot.nextRun = nextRun;
  if (report && text(report.id)) {
    bot.report = {
      id: text(report.id) ?? '',
      status: status(report.status),
      summary: text(report.summary) ?? '',
      at: text(report.at) ?? '',
      unread: report.unread === true,
    };
  }
  return bot;
}

function parseReport(value: unknown): MobileBotReport | undefined {
  const raw = record(value);
  const id = text(raw?.id);
  const handle = text(raw?.handle);
  if (!raw || !id || !handle) return undefined;
  const report: MobileBotReport = {
    id,
    handle,
    status: status(raw.status),
    summary: text(raw.summary) ?? '',
    items: list(raw.items).flatMap((item) => {
      const line = record(item);
      const words = text(line?.text);
      if (!line || words === undefined) return [];
      const url = openableLink(text(line.url));
      return [url ? { text: words, url } : { text: words }];
    }),
    at: text(raw.at) ?? '',
    read: raw.read === true,
    unread: raw.unread === true,
    repeat: typeof raw.repeat === 'number' && raw.repeat > 0 ? raw.repeat : 1,
  };
  const pane = text(raw.pane);
  if (pane) report.pane = pane;
  const job = text(raw.job);
  if (job) report.job = job;
  const reason = text(raw.reason);
  if (reason) report.reason = reason;
  const eventUrl = openableLink(text(raw.eventUrl));
  if (eventUrl) report.eventUrl = eventUrl;
  if (typeof raw.costUsd === 'number') report.costUsd = raw.costUsd;
  if (typeof raw.seconds === 'number') report.seconds = raw.seconds;
  if (raw.rerun === true) report.rerun = true;
  return report;
}

function parseApproval(value: unknown): MobileBotApproval | undefined {
  const raw = record(value);
  const id = text(raw?.id);
  if (!raw || !id) return undefined;
  const approval: MobileBotApproval = {
    id,
    rule: text(raw.rule) ?? '',
    type: text(raw.type) ?? '',
    subject: text(raw.subject) ?? '',
    what: text(raw.what) ?? '',
    at: text(raw.at) ?? '',
  };
  const workspace = text(raw.workspace);
  if (workspace) approval.workspace = workspace;
  const title = text(raw.title);
  if (title) approval.title = title;
  const url = openableLink(text(raw.url));
  if (url) approval.url = url;
  return approval;
}

/** `GET /m1/bots` read defensively: anything malformed is dropped rather than drawn; approvals the oldest first. */
export function parseBots(value: unknown): MobileBots {
  const raw = record(value);
  const reports = list(raw?.reports).flatMap((r) => parseReport(r) ?? []);
  const approvals = list(raw?.approvals)
    .flatMap((a) => parseApproval(a) ?? [])
    .sort((a, b) => a.at.localeCompare(b.at));
  return {
    protocol: typeof raw?.protocol === 'number' ? raw.protocol : 1,
    bots: list(raw?.bots).flatMap((b) => parseBot(b) ?? []),
    reports,
    approvals,
    unread: typeof raw?.unread === 'number' ? raw.unread : reports.filter((r) => r.unread).length,
  };
}

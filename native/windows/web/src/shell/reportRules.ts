// MC-73: the report box's rules, apart from its drawing (ReportBox.tsx): the reports filtered by status and bot, grouped by day
// (today, yesterday, then the date), the ones that need the person first within a day, and a day's ✅ ones folded into one line.

export type ReportStatus = 'ok' | 'report' | 'needs' | 'failed';

export interface BotReportItem {
  text: string;
  url?: string | null;
}

export interface BotReport {
  id: string;
  handle: string;
  pane?: string | null;
  status: ReportStatus;
  summary: string;
  items: BotReportItem[];
  /** RFC 3339. */
  at: string;
  record?: string | null;
  job?: string | null;
  reason?: string | null;
  costUsd?: number | null;
  seconds?: number | null;
  eventUrl?: string | null;
  read: boolean;
  thumb?: boolean | null;
  repeat: number;
}

export type ReportFilter = 'all' | ReportStatus;

export const statusMarks: Record<ReportStatus, string> = { ok: '✅', report: '📝', needs: '⚠️', failed: '❌' };
/** The order within a day: what needs the person, what failed, what reports, then the quiet ones. */
const rank: Record<ReportStatus, number> = { needs: 0, failed: 1, report: 2, ok: 3 };

export function filterReports(reports: readonly BotReport[], filter: ReportFilter, handle: string | null): BotReport[] {
  return reports.filter((r) => (filter === 'all' || r.status === filter) && (handle === null || r.handle === handle));
}

/** How many of each status, for the filter chips. */
export function statusCounts(reports: readonly BotReport[]): Record<ReportStatus, number> {
  const counts: Record<ReportStatus, number> = { ok: 0, report: 0, needs: 0, failed: 0 };
  for (const r of reports) counts[r.status] += 1;
  return counts;
}

/** A day's key: `today`, `yesterday`, else the local date (`YYYY-MM-DD`). */
export function dayKey(at: string, now: Date): string {
  const when = new Date(at);
  const local = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (local(when) === local(now)) return 'today';
  if (local(when) === local(yesterday)) return 'yesterday';
  return local(when);
}

export interface ReportDay {
  key: string;
  /** The reports to show as cards, the ones that need the person first. */
  shown: BotReport[];
  /** The day's ✅ ones, folded into one line (shown as cards when the filter asks for them). */
  quiet: BotReport[];
}

/** The reports by day, newest day first; within a day by status then newest first; ✅ folded unless `unfold`. */
export function groupByDay(reports: readonly BotReport[], now: Date, unfold: boolean): ReportDay[] {
  const days = new Map<string, BotReport[]>();
  for (const r of [...reports].sort((a, b) => b.at.localeCompare(a.at))) {
    const key = dayKey(r.at, now);
    days.set(key, [...(days.get(key) ?? []), r]);
  }
  return [...days.entries()].map(([key, list]) => {
    const sorted = [...list].sort((a, b) => rank[a.status] - rank[b.status] || b.at.localeCompare(a.at));
    return unfold ? { key, shown: sorted, quiet: [] } : { key, shown: sorted.filter((r) => r.status !== 'ok'), quiet: sorted.filter((r) => r.status === 'ok') };
  });
}

/** The clock time of a report (`HH:MM`). */
export function clockOf(at: string): string {
  const d = new Date(at);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** A run's length in words: `2분 14초`-like, from the given unit words. */
export function lengthWords(seconds: number, minute: (n: number) => string, second: (n: number) => string): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return m > 0 ? `${minute(m)} ${second(s)}` : second(s);
}

import { describe, expect, it } from 'vitest';
import { dayKey, filterReports, groupByDay, lengthWords, statusCounts, type BotReport } from './reportRules';

const at = (date: Date, hours: number, minutes = 0) => new Date(date.getFullYear(), date.getMonth(), date.getDate(), hours, minutes).toISOString();
const now = new Date(2026, 10, 25, 18, 0);
const yesterday = new Date(2026, 10, 24);
const report = (id: string, handle: string, status: BotReport['status'], when: string): BotReport => ({
  id, handle, status, summary: id, items: [], at: when, read: false, repeat: 1,
});
const reports = [
  report('a', '콩이', 'report', at(now, 9, 2)),
  report('b', '두부', 'needs', at(now, 10, 4)),
  report('c', '깜이', 'failed', at(now, 10, 30)),
  report('d', '보리', 'ok', at(now, 9)),
  report('e', '호두', 'ok', at(now, 11)),
  report('f', '보리', 'report', at(yesterday, 18)),
  report('g', '콩이', 'report', at(new Date(2026, 10, 20), 9)),
];

describe('the report box (MC-73)', () => {
  it('names days today, yesterday, else by date', () => {
    expect(dayKey(at(now, 1), now)).toBe('today');
    expect(dayKey(at(yesterday, 23), now)).toBe('yesterday');
    expect(dayKey(at(new Date(2026, 10, 20), 9), now)).toBe('2026-11-20');
  });

  it('groups by day, newest day first, what needs the person and what failed first, the quiet ones folded', () => {
    const days = groupByDay(reports, now, false);
    expect(days.map((d) => d.key)).toEqual(['today', 'yesterday', '2026-11-20']);
    expect(days[0]?.shown.map((r) => r.id)).toEqual(['b', 'c', 'a']);
    expect(days[0]?.quiet.map((r) => r.id)).toEqual(['e', 'd']);
    expect(groupByDay(reports, now, true)[0]?.shown.map((r) => r.id)).toEqual(['b', 'c', 'a', 'e', 'd']);
  });

  it('filters by status and bot and counts each status', () => {
    expect(filterReports(reports, 'report', null).map((r) => r.id)).toEqual(['a', 'f', 'g']);
    expect(filterReports(reports, 'all', '보리').map((r) => r.id)).toEqual(['d', 'f']);
    expect(statusCounts(reports)).toEqual({ ok: 2, report: 3, needs: 1, failed: 1 });
  });

  it("says a run's length in minutes and seconds", () => {
    const words = (s: number) => lengthWords(s, (n) => `${n}분`, (n) => `${n}초`);
    expect(words(134)).toBe('2분 14초');
    expect(words(9)).toBe('9초');
  });
});

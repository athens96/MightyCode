import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { ReportCard } from './ReportBox';
import reportBoxCss from './reportBox.css?raw';
import { dayKey, filterReports, groupByDay, lengthWords, noticeCounts, noticeDays, openingTab, statusCounts, type BotReport, type Notice } from './reportRules';

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

  it('marks an unread report card with a header dot rather than a colored left border', () => {
    const services = {
      bridge: { call: () => Promise.resolve(null) },
      store: {},
      t: translator([{ 'bot.report.unread': '안 읽은 리포트', 'bot.report.status.report': '리포트' }]),
    } as unknown as AppServices;
    const render = (item: BotReport) => renderToStaticMarkup(
      createElement(ServicesContext.Provider, { value: services }, createElement(ReportCard, {
        report: item,
        expanded: false,
        onToggle: () => {},
        onOpenPane: () => {},
        onGo: () => {},
        onRerun: () => {},
        onThumb: () => {},
        onOpenUrl: () => {},
      })),
    );

    const unreadHtml = render({ ...report('a', '콩이', 'report', at(now, 9, 2)), read: false });
    expect(unreadHtml).toContain('class="report-card status-report unread"');
    expect(unreadHtml).toContain('data-testid="report-card-unread"');
    expect(unreadHtml).toContain('aria-label="안 읽은 리포트"');

    const readHtml = render({ ...report('a', '콩이', 'report', at(now, 9, 2)), read: true });
    expect(readHtml).toContain('class="report-card status-report"');
    expect(readHtml).not.toContain('data-testid="report-card-unread"');

    const cardRules = [...reportBoxCss.matchAll(/\.report-card\b[^{]*\{[^}]*\}/g)].map((m) => m[0]).join('\n');
    expect(cardRules).not.toMatch(/\bborder-left\b|\bborder-inline-start\b/);
    expect(cardRules).not.toMatch(/box-shadow\s*:[^;]*\binset\s+[1-9]/);
  });
});

describe('the 알림 함 general notices (MC-90)', () => {
  const notice = (id: string, status: Notice['status'], when: string): Notice => ({
    id, pane: 'p-' + id, workspace: 'Shop', title: 'Claude', status, summary: 'done', at: when, read: false,
  });
  const list = [notice('a', 'ok', at(now, 9)), notice('b', 'failed', at(now, 11)), notice('c', 'ok', at(yesterday, 20))];

  it('groups by day, newest first, and filters by how it ended', () => {
    expect(noticeDays(list, 'all', now).map((d) => [d.key, d.notices.map((n) => n.id)])).toEqual([['today', ['b', 'a']], ['yesterday', ['c']]]);
    expect(noticeDays(list, 'failed', now).map((d) => d.notices.map((n) => n.id))).toEqual([['b']]);
    expect(noticeCounts(list)).toEqual({ ok: 2, failed: 1 });
  });

  it('opens on the general tab unless only the bots wait unread', () => {
    expect(openingTab(2, 5)).toBe('general');
    expect(openingTab(0, 3)).toBe('bots');
    expect(openingTab(0, 0)).toBe('general');
  });
});

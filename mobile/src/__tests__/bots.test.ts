import { createClient, type RelayChannel } from '@/api/client';
import type { MobileBot, MobileBots } from '@/api/types';
import {
  botTone,
  groupByKind,
  handleKey,
  inStateOrder,
  parseBots,
  reportTone,
  reportsOf,
  statusLine,
  timeWords,
  waitingCount,
} from '@/lib/bots';
import { resetLanguage, t } from '@/lib/i18n';
import { notificationPath } from '@/lib/push/notification';
import { applyBots, dropApproval, forgetHost, markBots, readReport, type BotsByHost } from '@/store/bots';

beforeAll(() => resetLanguage('ko'));
afterAll(() => resetLanguage());

function bot(overrides: Partial<MobileBot>): MobileBot {
  return {
    id: 'p1',
    handle: '리뷰어',
    kind: 'independent',
    state: 'idle',
    asleep: false,
    jobs: 0,
    jobNames: [],
    ...overrides,
  };
}

function payload(overrides: Partial<MobileBots> = {}): MobileBots {
  return { protocol: 1, bots: [], reports: [], approvals: [], unread: 0, ...overrides };
}

describe('the 봇 tab rules (botTab.ts mirrored)', () => {
  it('groups the independent bots first, then the workspace bots, each in state order', () => {
    const bots = [
      bot({ id: 'a', kind: 'workspace', state: 'idle' }),
      bot({ id: 'b', state: 'asleep' }),
      bot({ id: 'c', kind: 'workspace', state: 'needsYou' }),
      bot({ id: 'd', state: 'working' }),
      bot({ id: 'e', state: 'needsYou' }),
      bot({ id: 'f', kind: 'workspace', state: 'working' }),
    ];
    const groups = groupByKind(bots);
    expect(groups.map((g) => g.kind)).toEqual(['independent', 'workspace']);
    expect(groups[0]!.bots.map((b) => b.id)).toEqual(['e', 'd', 'b']);
    expect(groups[1]!.bots.map((b) => b.id)).toEqual(['c', 'f', 'a']);
    expect(groupByKind([bot({ kind: 'workspace' })]).map((g) => g.kind)).toEqual(['workspace']);
    expect(groupByKind([])).toEqual([]);
    expect(inStateOrder([bot({ id: 'x', state: 'idle' }), bot({ id: 'y', state: 'idle' })]).map((b) => b.id)).toEqual(['x', 'y']);
  });

  it('says in one line what a bot is doing, as the desktop line does', () => {
    expect(statusLine(bot({ state: 'needsYou', detail: '배포를 승인할까요?' }), undefined, t)).toBe('배포를 승인할까요?');
    expect(statusLine(bot({ state: 'needsYou' }), undefined, t)).toBe(t('bot.sidebar.line.needsYou'));
    expect(statusLine(bot({ state: 'working', detail: 'src 읽는 중' }), '09:00', t)).toBe('src 읽는 중');
    expect(statusLine(bot({ state: 'working' }), undefined, t)).toBe(t('bot.sidebar.line.working'));
    expect(statusLine(bot({ state: 'asleep' }), '09:00', t)).toBe(t('bot.sidebar.asleep'));
    expect(statusLine(bot({ state: 'idle' }), '09:00', t)).toBe(t('bot.sidebar.nextRun', { at: '09:00' }));
    expect(statusLine(bot({ state: 'idle' }), undefined, t)).toBe(t('bot.sidebar.independent'));
    expect(statusLine(bot({ state: 'idle', kind: 'workspace', title: '리뷰 창' }), undefined, t)).toBe('리뷰 창');
  });

  it('draws a state and a report as a dot tone, never an edge', () => {
    expect([botTone('needsYou'), botTone('working'), botTone('idle'), botTone('asleep')]).toEqual(['wait', 'run', 'idle', 'stop']);
    expect([reportTone('ok'), reportTone('report'), reportTone('needs'), reportTone('failed')]).toEqual(['done', 'run', 'wait', 'err']);
  });

  it('counts what waits for the person and finds a bot by its name as the desktop compares it', () => {
    expect(waitingCount(undefined)).toBe(0);
    const waiting = payload({
      bots: [bot({ state: 'needsYou' }), bot({ id: 'p2', state: 'working' })],
      approvals: [{ id: 'a1', rule: 'r', type: 't', subject: 's', what: 'w', at: '' }],
    });
    expect(waitingCount(waiting)).toBe(2);
    expect(handleKey(' @Reviewer ')).toBe('reviewer');
    const reports = parseBots({
      reports: [
        { id: 'r1', handle: 'Reviewer', status: 'ok', summary: '', items: [], at: '' },
        { id: 'r2', handle: '다른봇', status: 'ok', summary: '', items: [], at: '' },
      ],
    }).reports;
    expect(reportsOf(reports, '@reviewer').map((r) => r.id)).toEqual(['r1']);
  });

  it('says a next run as a clock today, else with its day', () => {
    const now = new Date(2026, 10, 26, 8, 0);
    expect(timeWords(new Date(2026, 10, 26, 9, 5).toISOString(), now, 'ko')).toMatch(/09:05/);
    expect(timeWords(new Date(2026, 10, 27, 9, 5).toISOString(), now, 'ko')).toMatch(/27.*09:05/);
    expect(timeWords('not a time', now)).toBeUndefined();
    expect(timeWords(undefined, now)).toBeUndefined();
  });
});

describe('reading GET /m1/bots', () => {
  it('keeps what is well formed, drops the rest, and opens only http(s) links', () => {
    const read = parseBots({
      protocol: 1,
      bots: [
        { id: 'p1', handle: '리뷰어', kind: 'workspace', workspace: 'web', state: 'needsYou', jobs: 2, jobNames: ['A', 3, 'B'] },
        { id: 'p2', handle: '문서', state: 'dancing', report: { id: 'r1', status: 'weird', summary: 'x', at: '', unread: true } },
        { handle: 'no id' },
        'junk',
      ],
      reports: [
        {
          id: 'r1',
          handle: '리뷰어',
          status: 'needs',
          summary: 'One risky PR.',
          items: [{ text: '#412', url: 'https://github.com/o/r/pull/412' }, { text: 'local', url: 'file:///etc/passwd' }, { url: 'x' }],
          at: '2026-11-26T09:00:00Z',
          eventUrl: 'javascript:alert(1)',
          unread: true,
          repeat: 0,
          rerun: true,
        },
        { id: 'r0' },
      ],
      approvals: [
        { id: 'late', rule: 'b', at: '2026-11-26T10:00:00Z', url: 'intent://x' },
        { id: 'early', rule: 'a', at: '2026-11-26T09:00:00Z', url: 'https://example.com/pr/1' },
      ],
    });
    expect(read.bots.map((b) => b.id)).toEqual(['p1', 'p2']);
    expect(read.bots[0]).toMatchObject({ kind: 'workspace', workspace: 'web', state: 'needsYou', jobNames: ['A', 'B'] });
    expect(read.bots[1]).toMatchObject({ kind: 'independent', state: 'idle', report: { status: 'report', unread: true } });
    expect(read.reports).toHaveLength(1);
    expect(read.reports[0]!.items).toEqual([{ text: '#412', url: 'https://github.com/o/r/pull/412' }, { text: 'local' }]);
    expect(read.reports[0]!.eventUrl).toBeUndefined();
    expect(read.reports[0]!.repeat).toBe(1);
    expect(read.approvals.map((a) => a.id)).toEqual(['early', 'late']);
    expect(read.approvals[0]!.url).toBe('https://example.com/pr/1');
    expect(read.approvals[1]!.url).toBeUndefined();
    expect(read.unread).toBe(1);
    expect(parseBots(null)).toEqual({ protocol: 1, bots: [], reports: [], approvals: [], unread: 0 });
  });
});

describe('the bots store', () => {
  const report = { id: 'r1', handle: '리뷰어', status: 'needs' as const, summary: 's', items: [], at: '', read: false, unread: true, repeat: 1 };

  it('takes a newer read and drops one older than what it holds', () => {
    let state: BotsByHost = {};
    state = applyBots(state, 'h', payload({ bots: [bot({})] }), 3);
    expect(state.h).toMatchObject({ revision: 3, loading: false, unsupported: false });
    const older = applyBots(state, 'h', payload(), 2);
    expect(older).toBe(state);
    state = applyBots(state, 'h', payload(), 4);
    expect(state.h!.data!.bots).toEqual([]);
  });

  it('marks a read under way, an error and an older desktop, keeping what it held', () => {
    let state = applyBots({}, 'h', payload({ bots: [bot({})] }), 1);
    state = markBots(state, 'h', { loading: true });
    expect(state.h!.loading).toBe(true);
    expect(markBots(state, 'h', { loading: true })).toBe(state);
    state = markBots(state, 'h', { loading: false, error: 'offline' });
    expect(state.h).toMatchObject({ error: 'offline', loading: false });
    expect(state.h!.data!.bots).toHaveLength(1);
    state = markBots(state, 'h', { error: undefined, unsupported: true });
    expect(state.h!.error).toBeUndefined();
    expect(state.h!.unsupported).toBe(true);
    expect(markBots({}, 'new', { unsupported: true }).new).toMatchObject({ unsupported: true, loading: false });
  });

  it('reads a report at once, on its list and on its bot row, and counts it off', () => {
    const held = applyBots(
      {},
      'h',
      payload({
        bots: [bot({ report: { id: 'r1', status: 'needs', summary: 's', at: '', unread: true } })],
        reports: [report],
        unread: 1,
      }),
      1,
    );
    const read = readReport(held, 'h', 'r1');
    expect(read.h!.data!.reports[0]).toMatchObject({ read: true, unread: false });
    expect(read.h!.data!.bots[0]!.report!.unread).toBe(false);
    expect(read.h!.data!.unread).toBe(0);
    expect(readReport(read, 'h', 'r1')).toBe(read);
    expect(readReport(read, 'h', 'gone')).toBe(read);
    expect(readReport({}, 'h', 'r1')).toEqual({});
  });

  it('drops a decided approval and forgets a host unpaired', () => {
    const approval = { id: 'a1', rule: 'r', type: 't', subject: 's', what: 'w', at: '' };
    const held = applyBots({}, 'h', payload({ approvals: [approval] }), 1);
    const decided = dropApproval(held, 'h', 'a1');
    expect(decided.h!.data!.approvals).toEqual([]);
    expect(dropApproval(decided, 'h', 'a1')).toBe(decided);
    expect(forgetHost(decided, 'h')).toEqual({});
    expect(forgetHost(decided, 'other')).toBe(decided);
  });
});

describe('the bot routes and pushes', () => {
  it('sends each action on its own path, a job only with run', async () => {
    const calls: { method: string; path: string; body?: unknown }[] = [];
    const channel: RelayChannel = {
      request: (method, path, body) => {
        calls.push({ method, path, body });
        return Promise.resolve({ status: 200, body: { protocol: 1, ok: true } });
      },
      onNotify: () => () => undefined,
    };
    const client = createClient(channel);
    await client.bots();
    await client.botAction('리뷰어', 'run', 'Daily');
    await client.botAction('리뷰어', 'sleep', 'Daily');
    await client.botReport('r 1', 'read');
    await client.botApproval('a1', 'approve');
    await client.botApproval('a1', 'skip');
    expect(calls).toEqual([
      { method: 'GET', path: '/m1/bots', body: undefined },
      { method: 'POST', path: `/m1/bots/${encodeURIComponent('리뷰어')}/run`, body: { job: 'Daily' } },
      { method: 'POST', path: `/m1/bots/${encodeURIComponent('리뷰어')}/sleep`, body: {} },
      { method: 'POST', path: '/m1/bot-reports/r%201/read', body: {} },
      { method: 'POST', path: '/m1/bot-approvals/a1/approve', body: {} },
      { method: 'POST', path: '/m1/bot-approvals/a1/skip', body: {} },
    ]);
  });

  it('opens the 봇 tab for an approval push and 봇 알림 for a report without a pane', () => {
    expect(notificationPath({ storageId: 'h', sessionId: 'bot-approval', kind: 'permission' })).toBe('/bots?view=bots');
    expect(notificationPath({ storageId: 'h', sessionId: 'bot-report', kind: 'failed' })).toBe('/bots?view=reports');
    expect(notificationPath({ storageId: 'h', sessionId: 'pane-1', kind: 'question' })).toBe('/host/h/session/pane-1');
  });
});

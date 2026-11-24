import { describe, expect, it } from 'vitest';
import { atWork, botCounts, botMatches, botState, groupByState, listedBots, statusLine } from './botTab';
import type { PaneInfo, SidebarBot } from './types';

const pane = (id: string, extra: Partial<PaneInfo> = {}): PaneInfo => ({
  id, workspaceId: 'w1', title: 'Claude ' + id, help: '', kind: 'claude', provider: 'claude', beta: false, tone: 'idle', word: '', settled: true,
  meta: '', tabMark: false, waiting: 0, dashMeta: '', activityError: false, ...extra,
});
const t = (key: string, subs?: Readonly<Record<string, string | number>>) => key + (subs ? JSON.stringify(subs) : '');

describe('the 봇 tab (MC-68)', () => {
  const a: SidebarBot = { id: 'a', handle: '깜이', workspace: 'Shop', workspaceId: 'w1' };
  const b: SidebarBot = { id: 'b', handle: '콩이', workspace: 'Shop', workspaceId: 'w1', instructed: true };
  const c: SidebarBot = { id: 'c', handle: '보리', folder: '/bots/보리' };
  const d: SidebarBot = { id: 'd', handle: '두부', workspace: 'Shop', workspaceId: 'w1', jobs: 2, nextRun: '2026-11-25T09:00:00+09:00' };
  const e: SidebarBot = { id: 'e', handle: '모모', workspace: 'Shop', workspaceId: 'w1', asleep: true, instructed: true };
  const bots = [a, b, c, d, e];
  const panes = new Map<string, PaneInfo>([
    ['a', pane('a', { tone: 'wait', attention: { kind: 'approval', text: '권한 요청', help: '승인: 권한 요청' } })],
    ['b', pane('b', { tone: 'run', activity: 'PR #12 리뷰 중' })],
    ['c', pane('c')],
    ['d', pane('d')],
    ['e', pane('e')],
  ]);

  it('knows a bot at work: independent, with instructions or with jobs', () => {
    expect(bots.map(atWork)).toEqual([false, true, true, true, true]);
  });

  it('puts waiting for the user before running, and running before asleep', () => {
    expect(bots.map((bot) => botState(bot, panes.get(bot.id)))).toEqual(['needsYou', 'working', 'idle', 'idle', 'asleep']);
    expect(botState({ ...a, asleep: true }, panes.get('a')), 'a sleeping bot asking still asks').toBe('needsYou');
  });

  it('lists the bots at work and any waiting for the user, the rest counted as hidden', () => {
    const plain: SidebarBot = { id: 'f', handle: '호두', workspace: 'Shop', workspaceId: 'w1' };
    expect(listedBots([...bots, plain], panes, false)).toEqual({ listed: bots, hidden: 1 });
    expect(listedBots([...bots, plain], panes, true).hidden).toBe(0);
  });

  it('groups by state in order, leaving empty states out', () => {
    expect(groupByState(bots, panes).map((g) => [g.state, g.bots.map((b) => b.handle)])).toEqual([
      ['needsYou', ['깜이']], ['working', ['콩이']], ['idle', ['보리', '두부']], ['asleep', ['모모']],
    ]);
    expect(botCounts(bots, panes)).toEqual({ needsYou: 1, working: 1 });
  });

  it('says in one line what each is doing', () => {
    expect(statusLine(a, panes.get('a'), undefined, t)).toBe('권한 요청');
    expect(statusLine(b, panes.get('b'), undefined, t)).toBe('PR #12 리뷰 중');
    expect(statusLine(b, pane('b', { tone: 'run' }), undefined, t)).toBe('bot.sidebar.line.working');
    expect(statusLine(c, panes.get('c'), undefined, t)).toBe('bot.sidebar.independent');
    expect(statusLine(d, panes.get('d'), '09:00', t)).toBe('bot.sidebar.nextRun{"at":"09:00"}');
    expect(statusLine(e, panes.get('e'), undefined, t)).toBe('bot.sidebar.asleep');
    expect(statusLine({ id: 'x', handle: 'x', workspaceId: 'w1' }, pane('x'), undefined, t)).toBe('Claude x');
  });

  it('searches the name, the workspace and the pane title', () => {
    expect(botMatches(a, panes.get('a'), ' 깜 ')).toBe(true);
    expect(botMatches(a, panes.get('a'), 'shop')).toBe(true);
    expect(botMatches(c, panes.get('c'), 'claude c')).toBe(true);
    expect(botMatches(c, panes.get('c'), 'zzz')).toBe(false);
  });
});

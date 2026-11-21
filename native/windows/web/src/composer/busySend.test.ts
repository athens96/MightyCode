import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import type { Pills } from './menus';
import { BusySendPill, busySendChoice, busySendRows, Toolbar } from './Toolbar';
import type { WorktreeAgent } from './types';

const t = translator([{
  'composer.busySend.title': 'While running, send to', 'composer.busySend.queue': 'Queue', 'composer.busySend.worktree': 'Worktree · {agent}',
  'composer.busySend.help': 'What Enter does while the pane runs',
}]);
const services = { bridge: { call: () => Promise.resolve(null) }, store: { dispatch: () => {} }, t } as unknown as AppServices;
const pills: Pills = {
  provider: 'claude', model: 'Default', modelTip: '', showsEffort: false, effort: '', effortEnabled: false, effortTip: '', permission: 'Ask', permissionHelp: '',
  unlocked: false, showsFast: false, fast: false, fastEnabled: false, moreOn: false, contextPercent: null, resuming: false,
};
const agents: WorktreeAgent[] = [
  { agent: 'claude', label: 'Claude', detail: 'Claude · Ask', enabled: true },
  { agent: 'codex', label: 'Codex', detail: 'Codex · Auto edits', enabled: true },
  { agent: 'gemini', label: 'Antigravity', detail: 'Antigravity — Not installed', enabled: false },
];
const render = (over: { busy?: boolean; worktreeAgents?: WorktreeAgent[]; busySend?: string | null } = {}) =>
  renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services },
    createElement(Toolbar, { paneId: 'p', pills, busy: false, showUsage: false, onUsageShown: () => {}, ...over })));
const pill = (html: string) => html.match(/<button[^>]*data-pill="busySend"[^>]*>/)?.[0] ?? '';

// 실행 중 보내기: what Enter does while the pane runs — the queue, or a new worktree of the chosen agent.
describe('the 실행 중 보내기 pill', () => {
  // The pill sits beside the worktree button (Composer.tsx, drawn while the pane runs), as an icon: the queue's, or the chosen agent's.
  const alone = (over: { agents?: WorktreeAgent[]; current?: string | null } = {}) =>
    renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services },
      createElement(BusySendPill, { paneId: 'p', agents: over.agents ?? agents, current: over.current ?? null, compact: true })));

  it('is no longer among the toolbar\'s pills, and draws nothing without a worktree agent', () => {
    expect(render({ busy: true, worktreeAgents: agents })).not.toContain('data-pill="busySend"');
    expect(alone({ agents: [] })).toBe('');
    const html = alone();
    expect(pill(html)).not.toContain('disabled');
    expect(pill(html)).toContain('aria-label="While running, send to · Queue"');
    expect(html).not.toContain('>Queue<');
  });

  it('shows the chosen agent\'s mark and names it in its label', () => {
    const html = alone({ current: 'codex' });
    expect(pill(html)).toContain('composer-busy-send compact on');
    expect(pill(html)).toContain('Worktree · Codex');
    expect(html).toContain('data-provider="codex"');
  });

  it('on the send button it is the split control\'s target: the mark and a chevron, never a squeezed disc or the agent\'s name', () => {
    const html = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services },
      createElement(BusySendPill, { paneId: 'p', agents, current: 'codex', split: true })));
    expect(pill(html)).toContain('composer-send-choose composer-busy-send on');
    expect(pill(html)).not.toContain('composer-pill');
    expect(pill(html)).not.toContain(' compact');
    expect(html).toContain('data-provider="codex"');
    expect(html).not.toContain('>Worktree · Codex<');
  });

  it('the old 보낼 대상 pill is gone', () => {
    expect(render({ busy: true, worktreeAgents: agents })).not.toContain('data-pill="ask"');
    expect(render()).not.toContain('data-pill="ask"');
  });
});

describe('the 실행 중 보내기 menu', () => {
  it('lists the queue first, then a worktree of each agent, off with its reason when it cannot run, the one in effect checked', () => {
    expect(busySendRows(agents, 'codex', t)).toEqual([
      { kind: 'header', text: 'While running, send to' },
      { kind: 'choice', text: 'Queue', enabled: true, checked: false },
      { kind: 'separator' },
      { kind: 'choice', text: 'Worktree · Claude', enabled: true, checked: false, help: 'Claude · Ask' },
      { kind: 'choice', text: 'Worktree · Codex', enabled: true, checked: true, help: 'Codex · Auto edits' },
      { kind: 'choice', text: 'Worktree · Antigravity', enabled: false, checked: false, help: 'Antigravity — Not installed' },
    ]);
    expect(busySendRows(agents, null, t)[1]).toEqual({ kind: 'choice', text: 'Queue', enabled: true, checked: true });
  });

  it('reads a row back as its agent, null for the queue', () => {
    expect(busySendChoice(agents, 1)).toBeNull();
    expect(busySendChoice(agents, 3)).toBe('claude');
    expect(busySendChoice(agents, 4)).toBe('codex');
    expect(busySendChoice(agents, 0)).toBeUndefined();
    expect(busySendChoice(agents, 2)).toBeUndefined();
    expect(busySendChoice(agents, 9)).toBeUndefined();
  });
});

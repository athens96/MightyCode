import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import type { Pills } from './menus';
import { busySendChoice, busySendRows, Toolbar } from './Toolbar';
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
  it('shows only while the pane runs and offers a worktree agent', () => {
    expect(render({ worktreeAgents: agents })).not.toContain('data-pill="busySend"');
    expect(render({ busy: true })).not.toContain('data-pill="busySend"');
    expect(render({ busy: true, worktreeAgents: [] })).not.toContain('data-pill="busySend"');
    const html = render({ busy: true, worktreeAgents: agents });
    expect(pill(html)).not.toContain('disabled');
    expect(html).toContain('>Queue<');
  });

  it('names and tints the agent chosen', () => {
    const html = render({ busy: true, worktreeAgents: agents, busySend: 'codex' });
    expect(pill(html)).toContain('composer-pill on');
    expect(html).toContain('>Worktree · Codex<');
    expect(html).toContain('data-provider="codex"');
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

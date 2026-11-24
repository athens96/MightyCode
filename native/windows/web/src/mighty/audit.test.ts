// MC-54: a request's audit: what it is made of (the run's saved entries, checkpoint, plan records, child panes and verification
// receipt), its order, its Markdown, and where it opens (the result block's button and menu).
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import ko from '@locales/ko.json';
import en from '@locales/en.json';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import type { AgentActivity, LogEntry, MightyGraphRun, PlanRecord, RunSession } from '../store/models';
import { emptyState, Store } from '../store/store';
import { auditMarkdown, buildTurnAudit, clock, codeSpan, firstLine, rowMeta, summaryLine } from './audit';
import { AuditDialog } from './AuditPanel';
import { Block, type BlockActions } from './Block';
import { nodeId } from './layout';
import type { VerifyReceipt } from './verify';
import auditSource from './audit.ts?raw';
import panelSource from './AuditPanel.tsx?raw';

const t = translator([ko as Record<string, string>]);
const tEn = translator([en as Record<string, string>]);

let serial = 0;
function tool(at: string, kind: string, summary: string, extra: Partial<AgentActivity> = {}): LogEntry {
  serial += 1;
  return {
    id: 'e' + serial, kind: 'output', text: summary, timestamp: at,
    activity: { id: 'a' + serial, provider: 'claude', kind, state: 'completed', summary, ...extra },
  };
}

const T = (s: number) => new Date(Date.UTC(2026, 10, 24, 6, 0, s)).toISOString();

function run(overrides: Partial<MightyGraphRun> = {}): MightyGraphRun {
  return {
    id: 'run-1', input: 'Fix the login bug\nand add a test', status: 'completed', finalOutput: 'Done',
    rootEntries: [
      { id: 'u', kind: 'user', text: 'Fix the login bug', timestamp: T(0) },
      tool(T(1), 'read', 'src/login.ts', { toolName: 'Read' }),
      tool(T(2), 'command', 'npm test', { toolName: 'Bash', durationMs: 1200 }),
      tool(T(5), 'edit', 'src/login.ts', { toolName: 'Edit' }),
      tool(T(6), 'web', 'https://example.com/docs', { toolName: 'WebFetch' }),
      tool(T(7), 'command', 'rm -rf build', { toolName: 'Bash', state: 'error', output: 'The user denied this tool request in Mighty Code.' }),
      tool(T(8), 'search', 'login', { toolName: 'Grep' }),
      tool(T(9), 'turn', 'Claude is working'),
      tool(T(10), 'tool', 'Fix the tests', { toolName: 'mcp__mighty-delegation__delegate' }),
      tool(T(11), 'tool', 'list issues', { toolName: 'mcp__linear__list_issues' }),
      tool(T(12), 'command', 'npm run deploy', { toolName: 'Bash', state: 'waiting' }),
    ],
    agents: [{ id: 'g1', title: 'Explore', input: 'look around', status: 'completed', entries: [tool(T(3), 'command', 'git status', { toolName: 'Bash' })] }],
    resultEntries: [{ id: 'r', kind: 'assistant', text: 'Done', timestamp: T(20) }],
    checkpoints: [
      { kind: 'start', at: T(0), tree: 't0', files: [] },
      { kind: 'end', at: T(19), tree: 't1', base: 't0', files: [
        { path: 'src/login.ts', status: 'M', added: 4, removed: 1 },
        { path: 'src/login.test.ts', status: 'A', added: 20, removed: 0 },
      ], more: 2 },
    ],
    ...overrides,
  };
}

const plan: PlanRecord = { id: 'p1', runId: 'run-1', plan: '1. fix', receivedAt: T(4), decidedAt: T(4), outcome: 'approvedConfirm', graphRunId: 'run-1' };

function pane(id: string, parent: string | undefined, createdAt: string, title: string): RunSession {
  return {
    id, workspaceId: 'w', title, kind: 'claude', status: 'idle', model: 'default', provider: 'codex', createdAt, draft: '', logs: [],
    settings: { effort: 'default', permissionMode: 'manual', maxTurns: null, maxBudgetUsd: null }, ...(parent ? { parentSessionId: parent } : {}),
  };
}

const receipt: VerifyReceipt = {
  id: 'v1', status: 'passed', rounds: 1, repairs: 0, maxRounds: 2, configHash: 'h', startedAt: T(21), finishedAt: T(25), runIds: ['run-1'],
  commands: [{ name: 'test', command: 'npm test', passed: true, durationMs: 3000 }, { name: 'lint', command: 'npm run lint', passed: false, exitCode: 2, durationMs: 400 }],
};

function audit() {
  return buildTurnAudit({
    run: run(), title: '요청 3 · Claude', paneId: 'pane-1', planHistory: [plan], receipt,
    sessions: [pane('child-1', 'pane-1', T(10), 'Fix the tests'), pane('child-2', 'pane-1', T(200), 'Later work'), pane('other', 'pane-9', T(10), 'Elsewhere')],
  }, t);
}

describe('a request audit', () => {
  it('lists what the request did in time order from its saved records', () => {
    const a = audit();
    expect(a.rows.map((r) => r.kind + ':' + r.text)).toEqual([
      'request:Fix the login bug',
      'command:npm test',
      'command:git status',
      'approval:계획 카드: 승인 · 매번 확인',
      'edit:src/login.ts',
      'web:https://example.com/docs',
      'approval:Bash · rm -rf build',
      'agent:Fix the tests',
      'child:Fix the tests',
      'tool:list issues',
      'command:npm run deploy',
      'file:src/login.ts',
      'file:src/login.test.ts',
      'file:그 밖에 파일 2개',
      'verify:npm test',
      'verify:npm run lint',
      'verify:검증 통과',
      'result:완료',
    ]);
    expect(a.reads).toBe(2);
    expect(a.title).toBe('요청 3 · Claude');
    expect(a.request).toBe('Fix the login bug');
    expect(a.counts).toEqual({ commands: 3, files: 2, web: 1, approvals: 2, agents: 2, tools: 1 });
  });

  it('marks a refused call as a denied approval, a waiting one as waiting, and who in the request did it', () => {
    const rows = audit().rows;
    const denied = rows.find((r) => r.kind === 'approval' && r.text.includes('rm -rf'))!;
    expect(denied.state).toBe('denied');
    expect(rowMeta(t, denied)).toContain('거부됨');
    const waiting = rows.find((r) => r.text === 'npm run deploy')!;
    expect(waiting.state).toBe('waiting');
    expect(rowMeta(t, waiting)).toEqual(['Bash', '승인 대기']);
    const sub = rows.find((r) => r.text === 'git status')!;
    expect(sub.who).toBe('Explore');
    expect(rowMeta(t, sub)).toEqual(['Bash', '완료', 'Explore에서']);
    expect(rowMeta(t, rows.find((r) => r.text === 'npm test' && r.kind === 'command')!)).toEqual(['Bash', '완료', '1.2s']);
    expect(rowMeta(t, rows.find((r) => r.text === 'src/login.ts' && r.kind === 'file')!)).toEqual(['수정 · +4 −1']);
    expect(rowMeta(t, rows.find((r) => r.text === 'npm run lint')!)).toEqual(['lint', '실패', '0.4s', '종료 코드 2']);
    expect(rowMeta(t, rows.find((r) => r.kind === 'child')!)).toEqual(['완료', 'codex 창']);
  });

  it('takes child panes only from the request’s own time, and a running request up to now', () => {
    const live = buildTurnAudit({
      run: run({ status: 'running', resultEntries: [] }), paneId: 'pane-1', sessions: [pane('child-2', 'pane-1', T(200), 'Later work')], now: Date.parse(T(300)),
    }, t);
    expect(live.rows.some((r) => r.kind === 'child' && r.text === 'Later work')).toBe(true);
    expect(live.rows[live.rows.length - 1]).toMatchObject({ kind: 'result', state: 'running', text: '진행 중' });
    expect(audit().rows.some((r) => r.text === 'Later work')).toBe(false);
  });

  it('is only the request and its result when the records hold nothing it did', () => {
    const empty = buildTurnAudit({ run: run({ rootEntries: [{ id: 'u', kind: 'user', text: 'Hi', timestamp: T(0) }], agents: [], checkpoints: undefined, input: 'Hi' }), paneId: 'p' }, t);
    expect(empty.rows.map((r) => r.kind)).toEqual(['request', 'result']);
    expect(summaryLine(t, empty)).toBe('명령 0 · 파일 0 · 웹 0 · 승인 0 · 위임 0');
  });

  it('copies as Markdown, one line a row with its code fenced past any backtick', () => {
    const markdown = auditMarkdown(tEn, buildTurnAudit({ run: run(), title: 'Request 3', paneId: 'pane-1', planHistory: [plan], receipt }, tEn), 'en');
    const lines = markdown.split('\n');
    expect(lines[0]).toBe('## Audit log · Request 3');
    expect(lines).toContain('> Fix the login bug');
    expect(markdown).toContain('Commands 3 · Files 2 · Web 1 · Approvals 2 · Delegations 1 · 2 read(s) and search(es) left out');
    expect(markdown).toMatch(/- \d\d:\d\d:\d\d · \*\*Command\*\* `npm test` — Bash · Done · 1\.2s/);
    expect(markdown).toContain('**Approval** `Bash · rm -rf build` — Denied');
    expect(markdown).toContain('**Changed file** `src/login.ts` — Modified · +4 −1');
    expect(markdown).toContain('**Verification** Verification passed');
    expect(markdown).not.toContain('**Request**');
    expect(codeSpan('echo `date`')).toBe('`` echo `date` ``');
    expect(codeSpan('a\nb')).toBe('`a ⏎ b`');
    expect(codeSpan('x'.repeat(20), 5)).toBe('`xxxxx…`');
  });

  it('says a time as a clock and nothing without one', () => {
    expect(clock(T(5), 'en')).toMatch(/^\d\d:\d\d:05$/);
    expect(clock(undefined, 'en')).toBe('');
    expect(firstLine('\n  first\nsecond')).toBe('first');
  });
});

describe('the audit panel and the result block', () => {
  const services = (call = vi.fn(() => Promise.resolve(null))) => ({ bridge: { call }, store: new Store(emptyState), t } as unknown as AppServices);

  it('draws each row with its kind, time and words, the filters and the copy button', () => {
    const html = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services() },
      createElement(AuditDialog, { audit: audit(), onCopy: () => Promise.resolve({ redacted: 0 }), onClose: () => {} })));
    expect(html).toContain('data-testid="turn-audit"');
    expect(html).toContain('감사 기록 · 요청 3 · Claude');
    expect(html).toContain('data-testid="turn-audit-row-request"');
    expect(html).toContain('rm -rf build');
    expect(html).toContain('data-testid="turn-audit-filter-command"');
    expect(html).toContain('data-testid="turn-audit-copy"');
    expect(html).toContain('명령 3 · 파일 2 · 웹 1 · 승인 2 · 위임 2');
  });

  it('opens from the result block’s header', () => {
    const actions = { openAudit: vi.fn(), copyAudit: vi.fn() } as unknown as BlockActions;
    const id = nodeId('run-1', 'result');
    const html = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services() }, createElement(Block, {
      paneId: 'p', provider: 'claude', id, kind: 'result', x: 0, y: 0, w: 400, h: 300, copy: undefined, extra: {}, visible: false,
      selected: false, expanded: false, running: false, actions,
    })));
    expect(html).toContain('data-testid="mighty-audit-' + id + '"');
    expect(html).toContain('title="감사 기록"');
    const request = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services() }, createElement(Block, {
      paneId: 'p', provider: 'claude', id: nodeId('run-1', 'request'), kind: 'request', x: 0, y: 0, w: 400, h: 300, copy: undefined, extra: {}, visible: false,
      selected: false, expanded: false, running: false, actions,
    })));
    expect(request).not.toContain('mighty-audit-');
  });

  it('keeps every word in the locales', () => {
    for (const source of [auditSource, panelSource]) {
      const code = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '').replace(/\/\*\*[\s\S]*?\*\//g, '');
      expect(code).not.toMatch(/[가-힣]/);
    }
  });
});

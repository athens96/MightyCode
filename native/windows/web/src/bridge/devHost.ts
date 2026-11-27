// A stand-in host for `npm run dev` in a plain browser: answers the page's methods, pushes a small fixture state, then
// changes as deltas made by the host's own rules (store/delta.ts), with the running pane streaming its last reply.
import { diffSession, type SessionDelta } from '../store/delta';
import type { AppSnapshot, LogEntry, RunSession } from '../store/models';
import type { HostPort } from './bridge';
import type { CallMessage, HostMessage } from './protocol';
import type { ShellModel } from '../shell/types';
import type { Criterion, LoopLimits, LoopRecord } from '../loops/types';
import { devWorkMap } from './devWorkMap';

const noCounts = { questions: 0, permissions: 0, errors: 0, running: 0, done: 0, stopped: 0, idle: 0 };

/** A frame model like the host's (MainWindow.WebShell.Shell.cs WebShellModel) for the fixture state, in English words. */
export function devShell(snapshot: AppSnapshot): ShellModel {
  const tone = (status: string) => (status === 'running' ? 'run' : status === 'completed' ? 'done' : status === 'error' ? 'err' : status === 'stopped' ? 'stop' : 'idle') as 'run';
  const ids = snapshot.sessions.filter((s) => s.workspaceId === snapshot.activeWorkspaceId).map((s) => s.id);
  const counts = { ...noCounts, running: snapshot.sessions.filter((s) => s.status === 'running').length, done: snapshot.sessions.filter((s) => s.status === 'completed').length };
  return {
    version: 'dev', dialogOpen: false, setupNeeded: false, entry: counts,
    panes: snapshot.sessions.map((s) => ({
      id: s.id, workspaceId: s.workspaceId, title: s.title, help: s.title, kind: s.kind, provider: s.provider, beta: s.provider !== 'claude', tone: tone(s.status), word: s.status,
      settled: s.status !== 'running', agent: s.provider, agentLabel: s.provider, meta: '· now', tabMark: s.status !== 'idle', waiting: 0, dashMeta: '· now', activityError: false,
    })),
    workspaces: snapshot.workspaces.map((w) => ({ id: w.id, name: w.name, path: w.path, local: true, counts, expanded: true, items: snapshot.sessions.filter((s) => s.workspaceId === w.id).map((s) => ({ type: 'pane' as const, id: s.id, depth: 0 })) })),
    header: snapshot.workspaces[0] && { id: snapshot.workspaces[0].id, name: snapshot.workspaces[0].name, path: snapshot.workspaces[0].path, filesEnabled: true, filesHelp: 'Files (Ctrl+Shift+E)', workMapEnabled: true, workMapHelp: 'Open Work Map' },
    layout: snapshot.activeWorkspaceId ? { workspaceId: snapshot.activeWorkspaceId, mode: 'tabs', root: { id: 'g1', kind: 'tabs', sessionIds: ids, selectedSessionId: snapshot.activeSessionId, ratio: 0.5, children: [] } } : undefined,
    status: { machine: '', panes: ids.length + ' panes', running: counts.running + ' running', updateHelp: '', setup: false, chipsTitle: 'Usage', chipsTooltip: 'Usage' },
  };
}

const at = '2026-10-24T00:00:00Z';

function pane(id: string, workspaceId: string, title: string, status: RunSession['status'], lines: number): RunSession {
  return {
    id, workspaceId, title, status, kind: 'claude', model: 'default', provider: 'claude', createdAt: at, draft: '',
    settings: { effort: 'default', permissionMode: 'manual', maxTurns: null, maxBudgetUsd: null },
    logs: Array.from({ length: lines }, (_, index) => ({ id: `${id}-${index}`, kind: index % 2 ? 'assistant' : 'user', text: `Line ${index + 1}`, timestamp: at })),
  };
}

/** One of each kind the conversation draws (the web smoke's fixture holds the same, MainWindow.WebShell.cs). */
export function fixtureEntries(prefix: string): LogEntry[] {
  return [
    { id: prefix + '-u', kind: 'user', text: 'Summarize the build and fix README.md:12', timestamp: at },
    { id: prefix + '-t1', kind: 'assistant', text: '', timestamp: at, activity: { id: 'a1', provider: 'claude', kind: 'command', state: 'completed', summary: 'npm run build', toolName: 'Bash', output: 'built in 1.2s', durationMs: 1234 } },
    { id: prefix + '-t2', kind: 'assistant', text: '', timestamp: at, activity: { id: 'a2', provider: 'claude', kind: 'read', state: 'error', summary: 'src/missing.ts', toolName: 'Read' } },
    {
      id: prefix + '-a', kind: 'assistant', timestamp: at,
      text: '## Result\n\nThe build **passed**; see `src/app/App.tsx:3` and [the docs](https://example.com).\n\n| file | lines |\n|:--|--:|\n| a.ts | 12 |\n| b.ts | 40 |\n\n- [x] build\n- [ ] deploy\n\n> quoted *note*\n\n```ts\nconst answer = 42;\n```',
    },
    { id: prefix + '-o', kind: 'output', text: 'PS> dir\n  a.txt\n  b.txt', timestamp: at },
    { id: prefix + '-s', kind: 'system', text: 'Model changed to default', timestamp: at },
    { id: prefix + '-e', kind: 'error', text: 'Request failed: network', timestamp: at },
  ];
}

export function devSnapshot(): AppSnapshot {
  return {
    version: 1, layout: 'grid', theme: 'dark', sidebarWidth: 252, sidebarCollapsed: false, statusLineEnabled: true, completionNotificationsEnabled: true,
    claudeDirectUsageLookupEnabled: false, appUpdateAutoCheck: true, languagePreference: 'system', settingsPane: 'general',
    mobileRemote: { enabled: false, relayURL: '', allowLegacyPhones: true, directEnabled: true, directPort: 0 },
    workspaces: [{ id: 'w1', name: 'Fixture project', path: 'fixture', createdAt: at }],
    sessions: [pane('s1', 'w1', 'Claude', 'completed', 80), pane('s2', 'w1', 'Codex', 'running', 3)],
    activeWorkspaceId: 'w1', activeSessionId: 's1',
  };
}

export function devLoops(): LoopRecord[] {
  const now = Date.now();
  const minsAgo = (m: number) => new Date(now - m * 60_000).toISOString();
  return [
    {
      id: 'loop-ci-214',
      paneId: 's2',
      workspaceId: 'w1',
      title: 'Fix PR 214 until CI is green',
      goal: 'Fix all failing unit tests and type errors on PR 214 without editing tests.',
      state: 'running',
      phase: 'repair',
      criteria: [
        {
          id: 'c-type',
          text: 'Typecheck passes',
          check: { kind: 'verifyConfig', name: 'typecheck', command: 'npx tsc --noEmit -p .', cwd: null },
          level: 'L1',
          status: 'passed',
          evidence: { level: 'L1', summary: '0 errors', at: minsAgo(2), durationMs: 1800 },
        },
        {
          id: 'c-test',
          text: 'Unit tests pass',
          check: { kind: 'command', name: 'test', command: 'npx vitest run', cwd: null },
          level: 'L2',
          status: 'failed',
          evidence: { level: 'L2', summary: '12 passed, 1 failed (crlf_last_line)', at: minsAgo(2), durationMs: 4100 },
        },
      ],
      criteriaHash: 'ab12cd34ef567890',
      limits: { maxIterations: 10, maxMinutes: 120, maxCostUsd: 5 },
      iterations: [
        { n: 1, kind: 'build', runId: 'run-1', startedAt: minsAgo(12), finishedAt: minsAgo(7), outcome: 'failed', failed: ['c-type', 'c-test'], changedFiles: 4, added: 40, removed: 11, costUsd: 0.62 },
        { n: 2, kind: 'repair', runId: 'run-2', startedAt: minsAgo(7), finishedAt: minsAgo(2), outcome: 'failed', failed: ['c-test'], changedFiles: 2, added: 18, removed: 6, costUsd: 0.45 },
        { n: 3, kind: 'repair', runId: 'run-3', startedAt: minsAgo(2), finishedAt: null, outcome: 'running', failed: [], changedFiles: 0, added: 0, removed: 0, costUsd: 0.13 },
      ],
      guard: { protectedFiles: 42, touched: [], testsAtStart: 310, testsNow: 310, skipsAdded: 0, flags: [] },
      costUsd: 1.20,
      startedAt: minsAgo(12),
      finishedAt: null,
      stopReason: null,
      baseCommit: 'a91c2e0f1234',
    },
    {
      id: 'loop-guard-1',
      paneId: 's1',
      workspaceId: 'w1',
      title: 'Apply PR review comments',
      goal: 'Address every review comment on the parser refactor and keep all tests intact.',
      state: 'paused',
      phase: 'verify',
      criteria: [
        {
          id: 'c-unit',
          text: 'All parser tests pass',
          check: { kind: 'command', name: 'test', command: 'npm test', cwd: null },
          level: 'L2',
          status: 'failed',
          evidence: { level: 'L2', summary: 'Guard blocked: protected test file edited', at: minsAgo(5), durationMs: 2400 },
        },
      ],
      criteriaHash: '1122334455667788',
      limits: { maxIterations: 6, maxMinutes: 60, maxCostUsd: 3 },
      iterations: [
        { n: 1, kind: 'build', runId: 'run-10', startedAt: minsAgo(15), finishedAt: minsAgo(5), outcome: 'failed', failed: ['c-unit'], changedFiles: 3, added: 22, removed: 9, costUsd: 0.54 },
      ],
      guard: { protectedFiles: 42, touched: ['src/parser.test.ts'], testsAtStart: 310, testsNow: 309, skipsAdded: 1, flags: ['protectedChanged', 'testsRemoved', 'skipAdded'] },
      costUsd: 0.54,
      startedAt: minsAgo(15),
      finishedAt: null,
      stopReason: null,
      baseCommit: 'b44d119a9988',
    },
    {
      id: 'loop-done-1',
      paneId: 's1',
      workspaceId: 'w1',
      title: 'Zero TypeScript errors',
      goal: 'Clean up remaining strict null checks.',
      state: 'passed',
      phase: 'done',
      criteria: [
        { id: 'c-tsc', text: 'Typecheck', check: { kind: 'verifyConfig', name: 'typecheck', command: 'npx tsc --noEmit', cwd: null }, level: 'L1', status: 'passed', evidence: { level: 'L1', summary: '0 errors', at: minsAgo(40), durationMs: 1200 } },
      ],
      criteriaHash: '9988776655443322',
      limits: { maxIterations: 8, maxMinutes: 60, maxCostUsd: 4 },
      iterations: [
        { n: 1, kind: 'build', runId: 'run-20', startedAt: minsAgo(55), finishedAt: minsAgo(48), outcome: 'failed', failed: ['c-tsc'], changedFiles: 5, added: 31, removed: 14, costUsd: 0.48 },
        { n: 2, kind: 'repair', runId: 'run-21', startedAt: minsAgo(48), finishedAt: minsAgo(40), outcome: 'passed', failed: [], changedFiles: 2, added: 6, removed: 4, costUsd: 0.32 },
      ],
      guard: { protectedFiles: 42, touched: [], testsAtStart: 310, testsNow: 310, skipsAdded: 0, flags: [] },
      costUsd: 0.80,
      startedAt: minsAgo(55),
      finishedAt: minsAgo(40),
      stopReason: 'passed',
      baseCommit: 'c77e301d2233',
    },
  ];
}

export function devHost(): HostPort {
  const listeners: ((event: { data: unknown }) => void)[] = [];
  const first = devSnapshot();
  let snapshot: AppSnapshot = { ...first, sessions: [{ ...first.sessions[0]!, logs: [...first.sessions[0]!.logs, ...fixtureEntries('f')] }, first.sessions[1]!] };
  let sent = snapshot;
  let streaming = false;
  // Off by default, as the contract says; Settings › 에이전트 (loopSetEnabled) turns it on.
  let loopsEnabled = false;
  let loops: LoopRecord[] = devLoops();
  let nextLoopSeq = 1;
  const send = (data: HostMessage) => setTimeout(() => listeners.forEach((listener) => listener({ data })), 0);
  const pushLoops = () => send({ kind: 'loops', enabled: loopsEnabled, loops: [...loops] });
  const flush = () => {
    const before = new Map(sent.sessions.map((session) => [session.id, session]));
    const sessions = snapshot.sessions.map((session) => diffSession(before.get(session.id), session)).filter((delta): delta is SessionDelta => delta !== null);
    const { sessions: _all, ...state } = snapshot;
    sent = snapshot;
    send({ kind: 'delta', sessions, removed: [], state });
    send({ kind: 'shell', shell: devShell(snapshot) });
  };
  const stream = () => {
    snapshot = {
      ...snapshot,
      sessions: snapshot.sessions.map((session) => {
        if (session.status !== 'running') return session;
        const last = session.logs[session.logs.length - 1]!;
        const grown: LogEntry = last.kind === 'assistant' && last.text.length < 600 ? { ...last, text: last.text + ' more words' } : { id: `${session.id}-${session.logs.length}`, kind: 'assistant', text: 'Streaming', timestamp: at };
        return { ...session, logs: grown.id === last.id ? [...session.logs.slice(0, -1), grown] : [...session.logs, grown] };
      }),
    };
    flush();
  };
  return {
    addEventListener: (_type, listener) => void listeners.push(listener),
    postMessage(message) {
      const call = message as CallMessage;
      const args = (call.args ?? {}) as Record<string, string | undefined>;
      if (call.method === 'selectSession') snapshot = { ...snapshot, activeSessionId: args.id };
      if (call.method === 'setTheme') snapshot = { ...snapshot, theme: args.theme ?? snapshot.theme };
      if (call.method === 'openWorkMap') {
        const id = 'workmap:w1';
        const opened = snapshot.sessions.some((session) => session.id === id) ? snapshot.sessions : [...snapshot.sessions, { ...pane(id, 'w1', 'Work map', 'idle', 0), kind: 'workmap' }];
        snapshot = { ...snapshot, sessions: opened, activeSessionId: id };
      }
      if (call.method === 'loopList') {
        send({ kind: 'result', id: call.id, ok: true, value: { enabled: loopsEnabled, loops: [...loops] } });
        return;
      }
      if (call.method === 'loopSetEnabled') {
        loopsEnabled = Boolean((call.args as { on?: boolean } | undefined)?.on);
        send({ kind: 'result', id: call.id, ok: true, value: { enabled: loopsEnabled } });
        pushLoops();
        return;
      }
      if (call.method === 'loopDraft') {
        const a = (call.args ?? {}) as { paneId?: string; goal?: string };
        const paneSession = snapshot.sessions.find((s) => s.id === a.paneId);
        if (!paneSession) {
          send({ kind: 'result', id: call.id, ok: false, error: 'no such pane' });
          return;
        }
        const goal = a.goal ?? '';
        const draft: LoopRecord = {
          id: 'draft',
          paneId: paneSession.id,
          workspaceId: paneSession.workspaceId,
          title: goal.split('\n')[0]?.slice(0, 80) || 'New loop',
          goal,
          state: 'draft',
          phase: 'build',
          criteria: [
            { id: 'c1', text: 'Typecheck passes', check: { kind: 'verifyConfig', name: 'typecheck', command: 'npx tsc --noEmit -p .', cwd: null }, level: 'L1', status: 'pending', evidence: null },
            { id: 'c2', text: 'Unit tests pass', check: { kind: 'verifyConfig', name: 'test', command: 'npx vitest run', cwd: null }, level: 'L2', status: 'pending', evidence: null },
            { id: 'c3', text: 'Checklist complete', check: { kind: 'checklist', name: 'checklist', command: null, cwd: null }, level: 'L1', status: 'pending', evidence: null },
          ],
          criteriaHash: null,
          limits: { maxIterations: 10, maxMinutes: 120, maxCostUsd: 5 },
          iterations: [],
          guard: { protectedFiles: 42, touched: [], testsAtStart: 310, testsNow: 310, skipsAdded: 0, flags: [] },
          costUsd: 0,
          startedAt: new Date().toISOString(),
          finishedAt: null,
          stopReason: null,
          baseCommit: 'a91c2e0f',
        };
        send({ kind: 'result', id: call.id, ok: true, value: draft });
        return;
      }
      if (call.method === 'loopStart') {
        const a = (call.args ?? {}) as { paneId?: string; goal?: string; criteria?: Criterion[]; limits?: LoopLimits };
        if (!loopsEnabled) { send({ kind: 'result', id: call.id, ok: false, error: 'loops are off' }); return; }
        const paneSession = snapshot.sessions.find((s) => s.id === a.paneId);
        if (!paneSession) { send({ kind: 'result', id: call.id, ok: false, error: 'no such pane' }); return; }
        if (loops.some((l) => l.paneId === paneSession.id && (l.state === 'running' || l.state === 'paused'))) {
          send({ kind: 'result', id: call.id, ok: false, error: 'the pane already runs a loop' });
          return;
        }
        const criteria = a.criteria ?? [];
        if (!criteria.some((c) => c.check.kind === 'command' || c.check.kind === 'verifyConfig')) {
          send({ kind: 'result', id: call.id, ok: false, error: 'no command criterion' });
          return;
        }
        const goal = (a.goal ?? '').trim();
        const created: LoopRecord = {
          id: `loop-dev-${nextLoopSeq++}`,
          paneId: paneSession.id,
          workspaceId: paneSession.workspaceId,
          title: goal.split('\n')[0]?.slice(0, 80) || 'Loop',
          goal,
          state: 'running',
          phase: 'build',
          criteria,
          criteriaHash: 'devhash012345678',
          limits: a.limits ?? { maxIterations: 10, maxMinutes: 120, maxCostUsd: 5 },
          iterations: [{ n: 1, kind: 'build', runId: 'run-dev-1', startedAt: new Date().toISOString(), finishedAt: null, outcome: 'running', failed: [], changedFiles: 0, added: 0, removed: 0, costUsd: 0 }],
          guard: { protectedFiles: 42, touched: [], testsAtStart: 310, testsNow: 310, skipsAdded: 0, flags: [] },
          costUsd: 0,
          startedAt: new Date().toISOString(),
          finishedAt: null,
          stopReason: null,
          baseCommit: 'a91c2e0f',
        };
        loops = [created, ...loops];
        send({ kind: 'result', id: call.id, ok: true, value: created });
        pushLoops();
        return;
      }
      if (call.method === 'loopPause' || call.method === 'loopResume' || call.method === 'loopStop') {
        const id = args.id;
        const found = loops.find((l) => l.id === id);
        if (!found) { send({ kind: 'result', id: call.id, ok: false, error: 'no such loop' }); return; }
        // Like the host: 잠시 멈춤 waits for the current turn (pausing, then paused a moment later); 계속 takes a pending pause back.
        const updated: LoopRecord = call.method === 'loopPause'
          ? (found.state === 'running' ? { ...found, pausing: true } : found)
          : call.method === 'loopResume'
          ? { ...found, state: 'running', pausing: false, phase: 'verify', guard: { ...found.guard, flags: [], touched: [] } }
          : { ...found, state: 'stopped', phase: 'done', stopReason: 'person', finishedAt: new Date().toISOString() };
        loops = loops.map((l) => (l.id === id ? updated : l));
        send({ kind: 'result', id: call.id, ok: true, value: updated });
        pushLoops();
        if (call.method === 'loopPause' && updated.pausing) {
          setTimeout(() => {
            const now = loops.find((l) => l.id === id);
            if (!now?.pausing) return;
            loops = loops.map((l) => (l.id === id ? { ...l, state: 'paused', pausing: false } : l));
            pushLoops();
          }, 1500);
        }
        return;
      }
      if (call.method === 'loopForget') {
        const id = args.id;
        loops = loops.filter((l) => l.id !== id);
        send({ kind: 'result', id: call.id, ok: true, value: {} });
        pushLoops();
        return;
      }
      const value = call.method === 'getLocale' ? { preference: 'system', language: navigator.language.split('-')[0] ?? 'en' } : call.method === 'shellState' ? devShell(snapshot) : call.method === 'workMapState' ? devWorkMap() : null;
      send({ kind: 'result', id: call.id, ok: true, value });
      if (call.method === 'ready') {
        sent = snapshot;
        send({ kind: 'snapshot', snapshot });
        pushLoops();
        if (!streaming) {
          streaming = true;
          setInterval(stream, 250);
        }
      }
      if (call.method === 'selectSession' || call.method === 'setTheme' || call.method === 'openWorkMap') flush();
    },
  };
}

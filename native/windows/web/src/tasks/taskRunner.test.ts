import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import type { AppSnapshot, RunSession } from '../store/models';
import { emptyState, Store } from '../store/store';
import { TaskRunnerPanel } from './TaskRunnerPanel';
import panelCssSource from './TaskRunnerPanel.css?raw';
import panelTsxSource from './TaskRunnerPanel.tsx?raw';
import modelTsSource from './taskRunnerModel.ts?raw';
import {
  FAILURE_TAIL_LINES,
  TASK_POLL_INTERVAL_MS,
  buildTaskFailurePrompt,
  filterTasks,
  findTargetAgentPane,
  handoffFailureToAgent,
  hasRunningTasks,
  isTaskBlockedByTrust,
  latestRunByTaskId,
  portBlockLabel,
  runLink,
  splitRunActions,
  tailLines,
  type RepoConfigStatus,
  type RepoTaskItem,
  type RepoTaskRun,
} from './taskRunnerModel';

const testTranslator = translator([
  {
    'dialog.close': 'Close',
    'dialog.resize': 'Resize',
    'dialog.loading': 'Loading...',
    'projectTasks.open': 'Run a task',
    'projectTasks.title': 'Tasks',
    'projectTasks.tasks': 'Project commands',
    'projectTasks.noTasks': 'No commands found.',
    'projectTasks.mightyConfig': 'Repository config (.mighty.yaml)',
    'projectTasks.trustPending': 'Approval required',
    'projectTasks.trustApproved': 'Approved',
    'projectTasks.approve': 'Approve',
    'projectTasks.revoke': 'Revoke',
    'projectTasks.runNow': 'Run',
    'projectTasks.runTerminal': 'Terminal',
    'projectTasks.stop': 'Stop',
    'projectTasks.running': 'Running',
    'projectTasks.succeeded': 'Succeeded',
    'projectTasks.failed': 'Failed',
    'projectTasks.askAgentFix': 'Send failure to agent',
    'projectTasks.fixPrompt': 'Task `{name}` (`{command}`) failed with exit code {code}. Please diagnose and fix it:',
    'projectTasks.worktreeRules': 'Worktree rules ({share} shared, {copy} copied)',
    'projectTasks.outputEmpty': 'Select and run a command to view live output.',
  },
]);

function makeSession(overrides: Partial<RunSession> & { id: string; workspaceId: string; kind: string }): RunSession {
  return {
    title: overrides.id,
    status: 'idle',
    model: 'default',
    provider: 'claude',
    settings: { effort: 'default', permissionMode: 'manual', maxTurns: null, maxBudgetUsd: null },
    logs: [],
    createdAt: '2026-11-22T00:00:00Z',
    draft: '',
    ...overrides,
  };
}

describe('taskRunnerModel', () => {
  it('polls at 1.2s ONLY when at least one task is running', () => {
    expect(TASK_POLL_INTERVAL_MS).toBe(1200);
    expect(hasRunningTasks([])).toBe(false);
    expect(
      hasRunningTasks([
        {
          runId: 'r1',
          taskId: 'package.json:test',
          name: 'test',
          command: 'pnpm run test',
          source: 'package.json',
          status: 'succeeded',
          exitCode: 0,
          output: 'ok',
          startedAt: '2026-11-22T00:00:00Z',
        },
        {
          runId: 'r2',
          taskId: 'Cargo.toml:check',
          name: 'check',
          command: 'cargo check',
          source: 'Cargo.toml',
          status: 'failed',
          exitCode: 101,
          output: 'err',
          startedAt: '2026-11-22T00:00:00Z',
        },
      ]),
    ).toBe(false);

    expect(
      hasRunningTasks([
        {
          runId: 'r3',
          taskId: 'mighty:ci',
          name: 'ci',
          command: 'pnpm ci',
          source: 'mighty',
          status: 'running',
          output: '',
          startedAt: '2026-11-22T00:00:00Z',
        },
      ]),
    ).toBe(true);
  });

  it('blocks mighty tasks until config is approved while allowing package.json/Cargo tasks', () => {
    const mightyTask: RepoTaskItem = {
      id: 'mighty:lint',
      name: 'lint',
      command: 'pnpm lint',
      source: 'mighty',
      requiresTrust: true,
    };
    const npmTask: RepoTaskItem = {
      id: 'package.json:test',
      name: 'test',
      command: 'pnpm run test',
      source: 'package.json',
    };
    const pendingCfg: RepoConfigStatus = {
      present: true,
      contentHash: 'a'.repeat(64),
      shortHash: 'a'.repeat(12),
      approved: false,
      shareCount: 1,
      copyCount: 1,
      setupCommand: 'pnpm install',
      files: ['.mighty.yaml'],
    };

    expect(isTaskBlockedByTrust(mightyTask, null)).toBe(true);
    expect(isTaskBlockedByTrust(mightyTask, pendingCfg)).toBe(true);
    expect(isTaskBlockedByTrust(mightyTask, { ...pendingCfg, approved: true })).toBe(false);
    expect(isTaskBlockedByTrust(npmTask, pendingCfg)).toBe(false);

    // A script a repository's Cargo.toml defines is an arbitrary shell command: it waits for the approval too.
    const cargoScript: RepoTaskItem = {
      id: 'Cargo.toml:lint-all',
      name: 'lint-all',
      command: 'cargo clippy',
      source: 'Cargo.toml',
      requiresTrust: true,
    };
    expect(isTaskBlockedByTrust(cargoScript, null)).toBe(true);
    expect(isTaskBlockedByTrust(cargoScript, pendingCfg)).toBe(true);
    expect(isTaskBlockedByTrust(cargoScript, { ...pendingCfg, approved: true })).toBe(false);
    expect(isTaskBlockedByTrust({ ...cargoScript, requiresTrust: false }, null)).toBe(false);
  });

  it('keeps latest run per taskId and filters tasks across fields', () => {
    const runs: RepoTaskRun[] = [
      {
        runId: 'newest',
        taskId: 'Cargo.toml:test',
        name: 'test',
        command: 'cargo test',
        source: 'Cargo.toml',
        status: 'failed',
        exitCode: 101,
        output: 'newest failure',
        startedAt: '2026-11-22T00:02:00Z',
      },
      {
        runId: 'older',
        taskId: 'Cargo.toml:test',
        name: 'test',
        command: 'cargo test',
        source: 'Cargo.toml',
        status: 'succeeded',
        exitCode: 0,
        output: 'old pass',
        startedAt: '2026-11-22T00:01:00Z',
      },
    ];
    expect(latestRunByTaskId(runs).get('Cargo.toml:test')?.runId).toBe('newest');

    const tasks: RepoTaskItem[] = [
      { id: 'mighty:db', name: 'db:migrate', command: 'just migrate', source: 'mighty', description: 'Database schema' },
      { id: 'Cargo.toml:clippy', name: 'clippy', command: 'cargo clippy -- -D warnings', source: 'Cargo.toml' },
    ];
    expect(filterTasks(tasks, 'cargo warnings')).toHaveLength(1);
    expect(filterTasks(tasks, 'database')[0]?.id).toBe('mighty:db');
  });

  it('formats failed task prompt with fenced 80-line tail and escapes inner backtick fences', () => {
    const lines = Array.from({ length: 100 }, (_, idx) =>
      idx === 95 ? '```ts\nconst broken = true;\n```' : `line-${idx + 1}`,
    ).join('\n');
    const tailed = tailLines(lines, FAILURE_TAIL_LINES);
    expect(tailed.split('\n').length).toBe(FAILURE_TAIL_LINES);
    expect(tailed).not.toContain('line-1\n');
    expect(tailed).toContain('line-100');

    const run: RepoTaskRun = {
      runId: 'run-fail',
      taskId: 'package.json:typecheck',
      name: 'typecheck',
      command: 'pnpm run typecheck',
      source: 'package.json',
      status: 'failed',
      exitCode: 2,
      output: lines,
      startedAt: '2026-11-22T00:00:00Z',
    };
    const prompt = buildTaskFailurePrompt(run, testTranslator);
    expect(prompt).toContain('Task `typecheck` (`pnpm run typecheck`) failed with exit code 2.');
    expect(prompt).toContain('````text\n');
    expect(prompt.endsWith('\n````')).toBe(true);
  });

  it('resolves target agent pane and dispatches fill + choose + activatePane on queueMicrotask', async () => {
    const shellPane = makeSession({ id: 'shell-1', workspaceId: 'ws-1', kind: 'shell' });
    const agentPane = makeSession({ id: 'agent-1', workspaceId: 'ws-1', kind: 'claude' });
    const otherWsPane = makeSession({ id: 'agent-2', workspaceId: 'ws-2', kind: 'claude' });
    const snapshot = {
      workspaces: [{ id: 'ws-1', name: 'App', path: '/repo', createdAt: '2026-11-22T00:00:00Z' }],
      sessions: [shellPane, otherWsPane, agentPane],
      activeWorkspaceId: 'ws-1',
      activeSessionId: 'shell-1',
    } as unknown as AppSnapshot;

    const target = findTargetAgentPane(snapshot, 'ws-1', 'shell-1');
    expect(target?.id).toBe('agent-1');

    const dispatches: unknown[] = [];
    const bridgeCalls: Array<{ method: string; args: unknown }> = [];
    let closed = false;

    const failedRun: RepoTaskRun = {
      runId: 'r-fail',
      taskId: 'Cargo.toml:test',
      name: 'test',
      command: 'cargo test',
      source: 'Cargo.toml',
      status: 'failed',
      exitCode: 101,
      output: 'assertion failed at src/lib.rs:10',
      startedAt: '2026-11-22T00:00:00Z',
    };

    handoffFailureToAgent({
      run: failedRun,
      target: target!,
      t: testTranslator,
      store: { dispatch: (msg) => dispatches.push(msg) },
      bridge: {
        call: <T = unknown>(method: string, args?: unknown): Promise<T> => {
          bridgeCalls.push({ method, args });
          return Promise.resolve(null as T);
        },
      },
      onClose: () => {
        closed = true;
      },
    });

    expect(closed).toBe(true);
    expect(dispatches).toHaveLength(0);

    await new Promise<void>((resolvePromise) => queueMicrotask(resolvePromise));

    expect(dispatches).toEqual([
      {
        kind: 'fill',
        paneId: 'agent-1',
        text: expect.stringContaining('assertion failed at src/lib.rs:10'),
        append: true,
      },
      { kind: 'choose', paneId: 'agent-1' },
    ]);
    expect(bridgeCalls).toEqual([{ method: 'activatePane', args: { id: 'agent-1' } }]);
  });
});

describe('TaskRunnerPanel rendering and design/locale guardrails', () => {
  it('renders nothing when closed and renders panel shell when open', () => {
    const store = new Store(emptyState);
    const services = {
      bridge: { call: <T = unknown>(): Promise<T> => Promise.resolve({ config: null, tasks: [], runs: [] } as T) },
      store,
      t: testTranslator,
    } as unknown as AppServices;

    const closedHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(TaskRunnerPanel, { open: false, onClose: () => {} }),
      ),
    );
    expect(closedHtml).toBe('');

    const openHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(TaskRunnerPanel, { open: true, workspaceId: 'ws-1', paneId: 'p-1', onClose: () => {} }),
      ),
    );
    expect(openHtml).toContain('data-testid="task-runner-panel"');
    expect(openHtml).toContain('Tasks');
    expect(openHtml).toContain('No commands found.');
    expect(openHtml).toContain('Select and run a command to view live output.');
  });

  it('contains zero raw color literals and zero hardcoded Hangul in production task runner files', () => {
    const sources: Array<[string, string]> = [
      ['taskRunnerModel.ts', modelTsSource],
      ['TaskRunnerPanel.tsx', panelTsxSource],
      ['TaskRunnerPanel.css', panelCssSource],
    ];
    const rawColorPattern = /#[0-9a-fA-F]{3,8}\b|\b(?:rgb|rgba|hsl|hsla)\s*\(/;
    const hangulPattern = /[\uAC00-\uD7A3\u3131-\u318E]/;

    for (const [name, source] of sources) {
      expect(source, `${name} must not contain raw color literals`).not.toMatch(rawColorPattern);
      expect(source, `${name} must not contain hardcoded Hangul`).not.toMatch(hangulPattern);
    }
  });
});

describe('run actions (MC-25)', () => {
  const action = (name: string, extra: Partial<RepoTaskItem> = {}): RepoTaskItem => ({
    id: 'run:' + name, name, command: 'npm run ' + name, source: 'run', requiresTrust: true, concurrent: true, url: 'auto', ...extra,
  });
  const run = (extra: Partial<RepoTaskRun>): RepoTaskRun => ({
    runId: 'r', taskId: 'run:dev', name: 'dev', command: 'npm run dev', source: 'run', status: 'running', output: '', startedAt: 't', ...extra,
  });

  it('splits run actions from the project commands and keeps their order', () => {
    const lint: RepoTaskItem = { id: 'mighty:lint', name: 'lint', command: 'npm run lint', source: 'mighty' };
    const split = splitRunActions([action('dev'), lint, action('e2e', { concurrent: false })]);
    expect(split.actions.map((a) => a.name)).toEqual(['dev', 'e2e']);
    expect(split.commands).toEqual([lint]);
    expect(isTaskBlockedByTrust(action('dev'), null)).toBe(true);
  });

  it('labels a port block and links only a running http address', () => {
    expect(portBlockLabel(41010, 10)).toBe(':41010\u201341019');
    expect(portBlockLabel(41010, 1)).toBe(':41010');
    expect(portBlockLabel(0, 10)).toBe('');
    expect(runLink(run({ url: 'http://localhost:41010/' }))).toBe('http://localhost:41010/');
    expect(runLink(run({ url: 'http://localhost:41010/', status: 'stopped' }))).toBeNull();
    expect(runLink(run({ url: 'javascript:alert(1)' }))).toBeNull();
    expect(runLink(null)).toBeNull();
  });
});

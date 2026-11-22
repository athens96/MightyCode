// Pure model types and helper logic for the Task Runner panel and `.mighty.yaml` trust/worktree summary (`MC-8`).
import type { Bridge } from '../bridge/bridge';
import type { Translate } from '../i18n/locale';
import type { AppSnapshot, RunSession } from '../store/models';
import type { Store } from '../store/store';

export const TASK_POLL_INTERVAL_MS = 1200;
export const FAILURE_TAIL_LINES = 80;

export type TaskSource = 'mighty' | 'package.json' | 'Cargo.toml' | 'Makefile' | 'justfile' | string;
export type TaskRunStatus = 'running' | 'succeeded' | 'failed' | 'stopped';

export interface RepoConfigStatus {
  present: true;
  contentHash: string;
  shortHash: string;
  approved: boolean;
  shareCount: number;
  copyCount: number;
  sharePaths?: string[];
  copyRules?: string[];
  setupCommand: string | null;
  envKeys?: string[];
  /** `KEY=value` for each variable `worktree.env` sets: what is approved is shown whole. */
  envLines?: string[];
  files: string[];
  error?: string | null;
}

export interface RepoTaskItem {
  id: string;
  name: string;
  command: string;
  description?: string | null;
  cwd?: string | null;
  source: TaskSource;
  requiresTrust?: boolean;
}

export interface RepoTaskRun {
  runId: string;
  taskId: string;
  name: string;
  command: string;
  source: TaskSource;
  workspaceId?: string;
  paneId?: string | null;
  status: TaskRunStatus;
  exitCode?: number | null;
  output: string;
  startedAt: string;
  endedAt?: string | null;
}

export interface RepoTasksListResponse {
  workspaceId?: string | null;
  paneId?: string | null;
  folder?: string | null;
  config: RepoConfigStatus | null;
  tasks: RepoTaskItem[];
  runs: RepoTaskRun[];
}

/** Whether polling should stay active (`1.2s` interval ONLY while at least one run is currently `'running'`). */
export function hasRunningTasks(runs: readonly RepoTaskRun[]): boolean {
  return runs.some((run) => run.status === 'running');
}

/** Maps each `taskId` to its most recent `RepoTaskRun` (preserving newest-first priority). */
export function latestRunByTaskId(runs: readonly RepoTaskRun[]): ReadonlyMap<string, RepoTaskRun> {
  const map = new Map<string, RepoTaskRun>();
  for (const run of runs) {
    if (!map.has(run.taskId)) {
      map.set(run.taskId, run);
    }
  }
  return map;
}

/** Filters discovered tasks by search query across name, command, source, and description. */
export function filterTasks(tasks: readonly RepoTaskItem[], query: string): RepoTaskItem[] {
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter((part) => part.length > 0);
  if (terms.length === 0) return [...tasks];
  return tasks.filter((task) => {
    const haystack = [task.name, task.command, task.source, task.description ?? '', task.cwd ?? '']
      .join(' ')
      .toLowerCase();
    return terms.every((term) => haystack.includes(term));
  });
}

/** Display badge text for a task's origin file. */
export function sourceBadgeLabel(source: TaskSource): string {
  return source === 'mighty' ? '.mighty.yaml' : source;
}

/** Whether running `task` is blocked until `.mighty.yaml` is approved. */
export function isTaskBlockedByTrust(task: RepoTaskItem, config: RepoConfigStatus | null): boolean {
  if (task.source !== 'mighty' && !task.requiresTrust) return false;
  return !config?.approved;
}

/** Extracts at most the last `maxLines` lines from `text` (`\r\n` normalized). */
export function tailLines(text: string, maxLines: number = FAILURE_TAIL_LINES): string {
  const normalized = text.replace(/\r\n/g, '\n').trimEnd();
  if (normalized.length === 0) return '';
  const lines = normalized.split('\n');
  return lines.slice(Math.max(0, lines.length - maxLines)).join('\n');
}

/** Wraps `content` in a Markdown code fence longer than any backtick run inside `content`. */
export function fenceCodeBlock(content: string, language = 'text'): string {
  let longest = 2;
  const matches = content.match(/`+/g);
  if (matches) {
    for (const run of matches) {
      if (run.length > longest) longest = run.length;
    }
  }
  const fence = '`'.repeat(longest + 1);
  return `${fence}${language}\n${content}\n${fence}`;
}

/** Builds the localized failure prompt with a fenced 80-line output tail for sending to an agent pane. */
export function buildTaskFailurePrompt(run: RepoTaskRun, t: Translate): string {
  const code = run.exitCode ?? -1;
  const header = t('projectTasks.fixPrompt', {
    name: run.name,
    command: run.command,
    code,
  });
  const tail = tailLines(run.output, FAILURE_TAIL_LINES);
  if (tail.length === 0) {
    return header;
  }
  return `${header}\n\n${fenceCodeBlock(tail)}`;
}

/** Resolves the best target agent pane (`kind === 'claude'`) in `workspaceId` to receive a failure fix prompt. */
export function findTargetAgentPane(
  snapshot: AppSnapshot | null,
  workspaceId: string | null | undefined,
  preferredPaneId?: string | null,
): RunSession | null {
  if (!snapshot) return null;
  const isAgentPane = (session: RunSession): boolean =>
    session.kind === 'claude' && (!workspaceId || session.workspaceId === workspaceId);

  if (preferredPaneId) {
    const preferred = snapshot.sessions.find((s) => s.id === preferredPaneId && isAgentPane(s));
    if (preferred) return preferred;
  }
  if (snapshot.activeSessionId) {
    const active = snapshot.sessions.find((s) => s.id === snapshot.activeSessionId && isAgentPane(s));
    if (active) return active;
  }
  if (workspaceId && snapshot.paneLayoutActiveSessionIds?.[workspaceId]) {
    const layoutActiveId = snapshot.paneLayoutActiveSessionIds[workspaceId];
    const layoutActive = snapshot.sessions.find((s) => s.id === layoutActiveId && isAgentPane(s));
    if (layoutActive) return layoutActive;
  }
  return snapshot.sessions.find(isAgentPane) ?? null;
}

/**
 * Closes the Task Runner panel and on `queueMicrotask` fills the target agent pane's composer,
 * chooses the pane immediately in store state, and calls `activatePane` on the host bridge.
 */
export function handoffFailureToAgent(options: {
  run: RepoTaskRun;
  target: RunSession;
  t: Translate;
  store: Pick<Store, 'dispatch'>;
  bridge: Pick<Bridge, 'call'>;
  onClose: () => void;
}): string {
  const { run, target, t, store, bridge, onClose } = options;
  const prompt = buildTaskFailurePrompt(run, t);
  onClose();
  queueMicrotask(() => {
    store.dispatch({ kind: 'fill', paneId: target.id, text: prompt, append: true });
    store.dispatch({ kind: 'choose', paneId: target.id });
    void bridge.call('activatePane', { id: target.id }).catch(() => {});
  });
  return prompt;
}

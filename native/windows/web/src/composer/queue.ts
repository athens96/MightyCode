// 바로 실행 on a queued row: what the host decided for it (Core QueueRunNow, M/QueueRunNow.swift) and how the button reads.

/** What 바로 실행 does on a queued row, as the host decided it (Core QueueRunNow, M/QueueRunNow.swift). */
export type RunNow = 'worktree' | 'agentMenu' | 'here' | 'steer' | 'held' | 'waitsForTurn' | 'waitsForHeld' | 'waitsForUpdate';

const kinds: readonly RunNow[] = ['worktree', 'agentMenu', 'here', 'steer', 'held', 'waitsForTurn', 'waitsForHeld', 'waitsForUpdate'];
/** The host's value, or null for anything else (no button). */
export function readRunNow(value: unknown): RunNow | null {
  return typeof value === 'string' && (kinds as readonly string[]).includes(value) ? (value as RunNow) : null;
}
/** The button's tooltip key and its values: which of a new worktree (of `agent`, the row's pick) or this pane happens, or why the row waits. */
export function runNowTip(runNow: RunNow, held: number, agent = ''): [string, Record<string, number | string>?] {
  switch (runNow) {
    case 'worktree':
    case 'agentMenu': return ['queue.runNowAgentHelp', { agent }];
    case 'steer': return ['queue.runNowSteerHelp'];
    case 'held': return ['queue.runNowHeldHelp', { count: held }];
    case 'waitsForTurn': return ['queue.runNowUnavailableHelp'];
    case 'waitsForHeld': return ['queue.runNowAfterHeldHelp'];
    case 'waitsForUpdate': return ['queue.runNowUpdateHelp'];
    default: return ['queue.runNowHereHelp'];
  }
}
/** A row that cannot run now: drawn disabled (aria-disabled, so its title still shows) and its click does nothing. */
export const runNowWaits = (runNow: RunNow): boolean => runNow.startsWith('waitsFor');
/**
 * The agent a worktree row runs with: the row's pick while it can run now, else the pane's own agent when it can, else the first
 * that can (a pane whose own agent cannot make a child in its mode); null when none can.
 */
export function rowAgent<T extends { agent: string; enabled: boolean }>(agents: readonly T[], picked: string | null, own: string): T | null {
  const usable = (id: string | null) => agents.find((a) => a.agent === id && a.enabled) ?? null;
  return usable(picked) ?? usable(own) ?? agents.find((a) => a.enabled) ?? null;
}

/** Worktree rows draw the branch, the rest the play triangle. */
export const runNowBranches = (runNow: RunNow): boolean => runNow === 'worktree' || runNow === 'agentMenu';

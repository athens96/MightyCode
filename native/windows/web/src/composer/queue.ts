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

/**
 * MC-49: a mirrored pane's queue is its host's. ×, 바로 실행, 고치기 and ↑/↓ go to the host, which runs a worktree row with the host pane's
 * own agent (no agent pick here). `actions` (× and 바로 실행), `edit` and `reorder` say which the host takes; an older host takes none
 * or only the first two, and the rest are not drawn.
 */
export interface QueueRemote {
  actions: boolean;
  edit?: boolean;
  reorder?: boolean;
}

/** What a queued row draws: 바로 실행 (null: none), the agent pick before a worktree row, ×, 고치기 and ↑/↓. */
export interface QueueRowButtons {
  runNow: RunNow | null;
  pick: boolean;
  remove: boolean;
  edit: boolean;
  move: boolean;
}
/** `locked`: the row's words are not edited (an allowed card's follow-up; a host's row whose words came cut). */
export function queueRowButtons(runNow: RunNow | null, remote: QueueRemote | null | undefined, locked = false): QueueRowButtons {
  if (!remote) return { runNow, pick: true, remove: true, edit: !locked, move: true };
  return {
    runNow: remote.actions ? runNow : null,
    pick: false,
    remove: remote.actions,
    edit: !!remote.edit && !locked,
    move: !!remote.reorder,
  };
}

/** ↑/↓ on a focused row (or its buttons): the place it moves to, or null when it is already at that end or the key is another. */
export function queueMoveTarget(index: number, count: number, key: string): number | null {
  if (key === 'ArrowUp') return index > 0 ? index - 1 : null;
  if (key === 'ArrowDown') return index < count - 1 ? index + 1 : null;
  return null;
}

/** 고치기's words as they would be saved: null when they cannot be (none on a row without files), as the host refuses them. */
export function editedWords(text: string, files: number): string | null {
  return text.trim().length === 0 && files === 0 ? null : text;
}

/** A host's row: whether 바로 실행 waits (a row that needs another agent's pick waits there), and its tooltip key and values. */
export function remoteRunNowWaits(runNow: RunNow): boolean {
  return runNowWaits(runNow) || runNow === 'agentMenu';
}
export function remoteRunNowTip(runNow: RunNow, held: number): [string, Record<string, number | string>?] {
  if (runNow === 'agentMenu') return ['queue.runNowUnavailableHelp'];
  if (runNow === 'worktree') return ['queue.runNowRemoteWorktreeHelp'];
  return runNowTip(runNow, held);
}

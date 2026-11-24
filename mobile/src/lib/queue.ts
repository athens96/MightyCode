import type { QueuedItem, QueuePlace } from '@/api/types';

/** Why a queued row's 바로 실행 is dimmed, as a word the phone puts into its own language. */
export type RunNowReason = 'turn' | 'held' | 'update' | 'agent';
export type RunNowRead = { kind: 'worktree' | 'here' } | { kind: 'unavailable'; reason: RunNowReason };

const reasons: readonly RunNowReason[] = ['turn', 'held', 'update', 'agent'];

/** The row's 바로 실행 as the host decided it. Any other `runNow` hides the button; an unknown reason reads as the current task. */
export function runNowOf(item: QueuedItem): RunNowRead | undefined {
  const value: unknown = item.runNow;
  if (value === 'worktree' || value === 'here') return { kind: value };
  if (value !== 'unavailable') return undefined;
  const reason: unknown = item.runNowReason;
  return { kind: 'unavailable', reason: (reasons as readonly unknown[]).includes(reason) ? (reason as RunNowReason) : 'turn' };
}

/** The locale key that says why a dimmed row cannot run now (the phone's own language, not the host's). */
export function runNowReasonKey(reason: RunNowReason): string {
  switch (reason) {
    case 'held':
      return 'queue.runNowAfterHeldHelp';
    case 'update':
      return 'queue.runNowUpdateHelp';
    case 'agent':
      return 'phone.queue.runNowAgentOnComputer';
    default:
      return 'queue.runNowUnavailableHelp';
  }
}

/**
 * MC-49: what a row offers on a host that advertised "queueEdit": 고치기 (not a held row nor an allowed card's follow-up) and ↑/↓
 * (not a held row). Without it the list reads as before.
 */
export function queueRowEdits(item: QueuedItem, editable: boolean): { edit: boolean; move: boolean } {
  const held = item.held === true;
  return { edit: editable && !held && item.locked !== true, move: editable && !held };
}

/**
 * MC-49: where ↑ or ↓ puts the row at `index`: just before the row above it, or before the row two below it (the end when there
 * is none). Undefined at either end, and above a held row (held rows go first and stay there).
 */
export function queueMovePlace(items: readonly QueuedItem[], index: number, direction: 'up' | 'down'): QueuePlace | undefined {
  if (direction === 'up') {
    const above = items[index - 1];
    return above && above.held !== true ? { before: above.id } : undefined;
  }
  if (index + 1 >= items.length) return undefined;
  const after = items[index + 2];
  return after ? { before: after.id } : { to: 1000 };
}

/** 고치기's words as they would be saved: undefined when they are none (the host refuses them). */
export function editedQueueWords(text: string): string | undefined {
  return text.trim().length === 0 ? undefined : text;
}

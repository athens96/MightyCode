// 바뀐 부분별 받기·버리기 (MC-30): what the hunk review works out without the browser. The host answers each file's hunks with a
// fingerprint, the decision kept for it and why it cannot be put back (mighty-files hunks.rs and revert.rs): `runChangesHunks` /
// `runChangesHunk` for a request (files.rs), `paneWorktreeHunks` / `paneWorktreeHunk` / `paneWorktreeReview` for a worktree's
// changes sheet (worktree_hunks.rs). DiffView draws the 받기 ✓ / 버리기 ↶ head of each hunk; the sheets hold the state.
import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { FileChanges } from './types';

/** Why a hunk (or the whole file) cannot be put back now. */
export type HunkRefusal = 'laterRun' | 'missing' | 'unreadable' | 'unchanged' | 'conflict';

/** Why a hunk cannot be put back, in words (the title) and short (the chip on its head). */
export const RefusalText: Record<HunkRefusal, string> = {
  laterRun: 'hunks.refusal.laterRun', missing: 'hunks.refusal.missing', unreadable: 'hunks.refusal.unreadable',
  unchanged: 'hunks.refusal.unchanged', conflict: 'hunks.refusal.conflict',
};
export const RefusalChip: Record<HunkRefusal, string> = {
  laterRun: 'hunks.refusalShort.laterRun', missing: 'hunks.refusalShort.missing', unreadable: 'hunks.refusalShort.unreadable',
  unchanged: 'hunks.refusalShort.unchanged', conflict: 'hunks.refusalShort.conflict',
};

/** One hunk of the file as the host reads it now. */
export interface HunkState {
  fingerprint: string;
  /** 받기: the person took it (a hunk put back is no longer in the file). */
  decision?: 'accepted';
  refusal?: HunkRefusal;
}

/** One file's hunk review (`runChangesHunks`, `paneWorktreeHunks`). */
export interface HunkReview {
  /** The diff the hunks are of; absent when the file cannot be reviewed (`refusal`). */
  changes?: FileChanges;
  hunks: HunkState[];
  /** Hunks taken or put back, of all the file had (those put back included). */
  handled: number;
  total: number;
  refusal?: HunkRefusal;
}

/** How a put-back went (`outcome` of a discard). */
export type HunkOutcome = { kind: 'reverted' } | { kind: 'refused'; refusal: HunkRefusal } | { kind: 'failed'; error: string };

/** What the hunk head and the keys do. */
export type HunkAction = 'accept' | 'discard';

/** A file's progress: handled of total, and 검토됨 when every hunk was taken or put back. */
export interface HunkProgress { handled: number; total: number; reviewed: boolean }

export function isHandled(state: HunkState | undefined): boolean {
  return state?.decision === 'accepted';
}

/** Whether a review has hunks to act on (a diff with hunks, no refusal for the whole file). */
export function reviewable(review: HunkReview | null | undefined): review is HunkReview & { changes: FileChanges } {
  return !!review && !review.refusal && !!review.changes && !review.changes.unchanged && review.changes.hunks.length > 0;
}

/** Whether the hunk at `index` may be acted on: `accept` while not taken, `discard` while not refused. */
export function canAct(state: HunkState | undefined, action: HunkAction): boolean {
  if (!state) return false;
  return action === 'accept' ? !isHandled(state) : state.refusal === undefined;
}

/** The first hunk not taken yet at or after `from` (wrapping round), else `from` clamped to the list. */
export function firstOpen(states: readonly HunkState[], from = 0): number {
  if (states.length === 0) return 0;
  for (let step = 0; step < states.length; step++) {
    const index = (from + step) % states.length;
    if (!isHandled(states[index])) return index;
  }
  return Math.min(Math.max(from, 0), states.length - 1);
}

/** `]` and `[`: the next or previous hunk, staying at the ends. */
export function stepHunk(count: number, current: number, direction: 1 | -1): number {
  if (count === 0) return 0;
  return Math.min(Math.max(current + direction, 0), count - 1);
}

/** The current hunk once the review was read again: the same hunk when it is still there, else the first open one from where it was. */
export function keepCurrent(before: readonly HunkState[], after: readonly HunkState[], current: number): number {
  const print = before[current]?.fingerprint;
  const same = print === undefined ? -1 : after.findIndex((state) => state.fingerprint === print);
  if (same >= 0 && !isHandled(after[same])) return same;
  return firstOpen(after, Math.min(same >= 0 ? same : current, Math.max(after.length - 1, 0)));
}

/** What a key does in the review (none while typing, or with a modifier held). */
export function hunkKey(event: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'altKey'> & { target: EventTarget | null }): 'next' | 'previous' | HunkAction | null {
  if (event.ctrlKey || event.metaKey || event.altKey) return null;
  const target = event.target as { tagName?: string; type?: string; isContentEditable?: boolean } | null;
  const tag = target?.tagName?.toLowerCase();
  const typing = tag === 'input' ? !['checkbox', 'radio', 'button'].includes(target?.type ?? '') : tag === 'textarea' || tag === 'select';
  if (typing || target?.isContentEditable) return null;
  switch (event.key) {
    case ']': return 'next';
    case '[': return 'previous';
    case 'a': case 'A': return 'accept';
    case 'x': case 'X': return 'discard';
    default: return null;
  }
}

/** The file list's progress text: `n/m`, nothing for a file with no hunks. */
export function progressText(progress: Pick<HunkProgress, 'handled' | 'total'> | undefined): string {
  return progress && progress.total > 0 ? `${progress.handled}/${progress.total}` : '';
}

/**
 * The review's current hunk and its keys: `]` / `[` move, `a` takes the current hunk, `x` puts it back (`act`). The current hunk is
 * kept across a new reading of the same `file` when it is still there; another file starts at its first open hunk.
 */
export function useHunkReview(file: string | null, review: HunkReview | null, act: (action: HunkAction, index: number) => void) {
  const [current, setCurrent] = useState(0);
  const last = useRef<{ file: string | null; states: readonly HunkState[] }>({ file: null, states: [] });
  const next = review?.hunks;
  useEffect(() => {
    const after = next ?? [];
    const before = last.current.file === file ? last.current.states : [];
    last.current = { file, states: after };
    setCurrent((now) => (before.length === 0 ? firstOpen(after) : keepCurrent(before, after, now)));
  }, [file, next]);
  const onKeyDown = useCallback((event: KeyboardEvent) => {
    if (!reviewable(review)) return;
    const what = hunkKey(event);
    if (!what) return;
    event.preventDefault();
    event.stopPropagation();
    if (what === 'next' || what === 'previous') setCurrent((now) => stepHunk(review.hunks.length, now, what === 'next' ? 1 : -1));
    else if (canAct(review.hunks[current], what)) act(what, current);
  }, [review, current, act]);
  return { current, setCurrent, onKeyDown };
}

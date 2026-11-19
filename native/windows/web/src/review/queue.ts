// 검토 대기열 (src-tauri review.rs): the model the host keeps of everything ready to review, the push that brings it up to date, the
// panel's open state, and what each item's actions call. The host reads git only when the queue is refreshed (a run ended, the panel
// opened, its refresh button, after an action), so the page holds the last model and draws it.
import { useSyncExternalStore } from 'react';

export type ReviewKind = 'worktree' | 'child' | 'request';
export type ReviewStatus = 'ready' | 'conflict' | 'merged';

/** One item (review.rs `model`). */
export interface ReviewItem {
  /** What `reviewDismiss` hides it by (its counts are part of it: new work shows it again). */
  key: string;
  kind: ReviewKind;
  /** The worktree pane's or child's id (a child's record id is its pane's); a request's pane. */
  id: string;
  paneId: string;
  /** A request's graph run (요청 변경 검토 opens on it). */
  runId?: string;
  title: string;
  /** A request's words, on one line. */
  request?: string;
  agent: string;
  workspace: string;
  branch?: string;
  base?: string;
  status: ReviewStatus;
  busy: boolean;
  files: number;
  added: number;
  removed: number;
  ahead?: number;
  behind?: number;
  uncommitted?: number;
  canMerge: boolean;
  canCommitMerge: boolean;
  canPr: boolean;
}

export interface ReviewQueue {
  items: ReviewItem[];
  /** What is not merged yet: the sidebar's badge. */
  count: number;
}

/** host → page: the queue changed. */
export interface ReviewPush {
  kind: 'review';
  queue: ReviewQueue;
}

export function isReviewPush(message: { kind: string }): message is ReviewPush {
  return message.kind === 'review' && typeof (message as { queue?: unknown }).queue === 'object' && (message as { queue?: unknown }).queue !== null;
}

let queue: ReviewQueue | null = null;
let open = false;
const listeners = new Set<() => void>();
const notify = () => { for (const listener of listeners) listener(); };
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };

/** The queue as the host answered or pushed it last. */
export function setReviewQueue(next: ReviewQueue | null | undefined): void {
  if (!next || !Array.isArray(next.items)) return;
  queue = next;
  notify();
}

export function dispatchReview(message: ReviewPush): void {
  setReviewQueue(message.queue);
}

export function useReviewQueue(): ReviewQueue | null {
  return useSyncExternalStore(subscribe, () => queue, () => queue);
}

export function setReviewOpen(next: boolean): void {
  if (open === next) return;
  open = next;
  notify();
}

export function toggleReview(): void {
  setReviewOpen(!open);
}

export function useReviewOpen(): boolean {
  return useSyncExternalStore(subscribe, () => open, () => open);
}

export type ReviewAction = 'changes' | 'merge' | 'commitMerge' | 'discard' | 'editor' | 'pr' | 'dismiss' | 'focus';

/**
 * The actions an item offers, in the row's order: 변경 보기 always; 커밋하고 병합 while a worktree pane has files not committed, else
 * 병합 (a worktree pane or child not merged yet); 편집기에서 열기; PR 열기 for a worktree pane (push and GitHub's compare page);
 * 버리기 for a worktree; then 창으로 가기 and 검토 완료.
 */
export function itemActions(item: ReviewItem): ReviewAction[] {
  const actions: ReviewAction[] = ['changes'];
  if (item.canCommitMerge) actions.push('commitMerge');
  else if (item.canMerge) actions.push('merge');
  actions.push('editor');
  if (item.canPr) actions.push('pr');
  if (item.kind !== 'request') actions.push('discard');
  actions.push('focus', 'dismiss');
  return actions;
}

type Call = (method: string, args?: unknown) => Promise<unknown>;

export interface ReviewHooks {
  /** 요청 변경 검토 opened on the request (the page's own panel). */
  openRunChanges(paneId: string, runId: string): void;
}

/**
 * Runs one action through the method that already does it (their confirmations included): a worktree pane's bar
 * (`paneWorktreeAction`), a child's card (`childAction`), the child's changes sheet (`reviewChanges`), `openInEditor` (the worktree's
 * folder, a request's run folder), `selectSession`, `reviewDismiss`. Answers whether the queue should be read again afterwards.
 */
export async function runReviewAction(call: Call, item: ReviewItem, action: ReviewAction, hooks: ReviewHooks): Promise<boolean> {
  switch (action) {
    case 'changes':
      if (item.kind === 'request') { if (item.runId) hooks.openRunChanges(item.paneId, item.runId); return false; }
      if (item.kind === 'child') await call('reviewChanges', { id: item.id });
      else await call('paneWorktreeAction', { id: item.id, action: 'changes' });
      return false;
    case 'merge': case 'discard':
      if (item.kind === 'child') await call('childAction', { id: item.id, action });
      else if (item.kind === 'worktree') await call('paneWorktreeAction', { id: item.id, action });
      return true;
    case 'commitMerge': case 'pr':
      if (item.kind === 'worktree') await call('paneWorktreeAction', { id: item.id, action });
      return true;
    case 'editor':
      if (item.kind === 'request') await call('openInEditor', { source: 'reference', paneId: item.paneId, path: '' });
      else await call('openInEditor', { source: 'worktree', id: item.id, path: '' });
      return false;
    case 'focus':
      await call('selectSession', { id: item.paneId });
      return false;
    case 'dismiss': {
      const next = await call('reviewDismiss', { key: item.key });
      setReviewQueue(next as ReviewQueue | null);
      return false;
    }
  }
}

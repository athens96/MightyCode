// 검토 대기열 (queue.ts, ReviewPanel.tsx; src-tauri review.rs): the push, each item's actions and the methods they call, the panel's
// list as drawn, and the shortcut and menu item that open it.
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { isMenuPush } from '../shell/menu';
import { macShortcutAction, shortcutAction } from '../shell/shortcuts';
import { ReviewList } from './ReviewPanel';
import { isReviewPush, itemActions, runReviewAction, setReviewQueue, type ReviewItem, type ReviewQueue } from './queue';

const words: Record<string, string> = {
  'review.empty': 'Nothing to review.', 'review.files': '{count} files', 'review.status.ready': 'Ready', 'review.status.conflict': 'Conflict',
  'review.status.merged': 'Merged', 'review.kind.worktree': 'Worktree pane', 'review.kind.child': 'Child pane', 'review.kind.request': 'Request',
  'review.action.changes': 'View Changes', 'review.action.merge': 'Merge', 'review.action.commitMerge': 'Commit and Merge',
  'review.action.discard': 'Discard', 'review.action.pr': 'Open PR', 'review.action.dismiss': 'Mark Reviewed', 'review.action.focus': 'Go to Pane',
  'review.action.dismissHelp': 'Hide', 'editor.open': 'Open in Editor', 'dialog.loading': 'Loading…',
};
const t = translator([words]);
const services = { bridge: { call: () => new Promise(() => {}) }, store: { dispatch: () => {} }, t } as unknown as AppServices;
const render = (element: ReturnType<typeof createElement>) => renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, element));

const item = (over: Partial<ReviewItem>): ReviewItem => ({
  key: 'worktree:a:ready:2:3:12:4', kind: 'worktree', id: 'a', paneId: 'a', title: 'Login fix', agent: 'Claude', workspace: 'app',
  branch: 'mighty/a', base: 'main', status: 'ready', busy: false, files: 3, added: 12, removed: 4, uncommitted: 1,
  canMerge: true, canCommitMerge: true, canPr: true, ...over,
});

const child = item({ key: 'child:c:conflict:1:1:3:1', kind: 'child', id: 'c', paneId: 'c', title: 'Refactor', agent: 'Codex', status: 'conflict', canCommitMerge: false, canPr: false });
const request = item({ key: 'request:p:run-1', kind: 'request', id: 'p', paneId: 'p', runId: 'run-1', request: 'fix the bug', branch: undefined, base: undefined, canMerge: false, canCommitMerge: false, canPr: false });
const merged = item({ key: 'worktree:m:merged:0:0:0:0', id: 'm', paneId: 'm', status: 'merged', files: 0, added: 0, removed: 0, canMerge: false, canCommitMerge: false, canPr: false });

/** Every call an action made, in order. */
function recorder(answer: unknown = null) {
  const calls: [string, unknown][] = [];
  const call = (method: string, args?: unknown) => { calls.push([method, args]); return Promise.resolve(answer); };
  return { calls, call };
}

describe('the review queue (review/queue.ts)', () => {
  it('takes the host\'s push and nothing else', () => {
    expect(isReviewPush({ kind: 'review', queue: { items: [], count: 0 } } as never)).toBe(true);
    expect(isReviewPush({ kind: 'review' })).toBe(false);
    expect(isReviewPush({ kind: 'files' })).toBe(false);
  });

  it('offers each kind its actions: commit and merge while files are not committed, merge else, PR for a worktree pane only', () => {
    expect(itemActions(item({}))).toEqual(['changes', 'commitMerge', 'editor', 'pr', 'discard', 'focus', 'dismiss']);
    expect(itemActions(item({ canCommitMerge: false }))).toEqual(['changes', 'merge', 'editor', 'pr', 'discard', 'focus', 'dismiss']);
    expect(itemActions(child)).toEqual(['changes', 'merge', 'editor', 'discard', 'focus', 'dismiss']);
    expect(itemActions(request)).toEqual(['changes', 'editor', 'focus', 'dismiss']);
    expect(itemActions(merged)).toEqual(['changes', 'editor', 'discard', 'focus', 'dismiss']);
  });

  it('runs a worktree pane\'s actions through its bar, a child\'s through its card, and reads the queue again after a git step', async () => {
    const opened: [string, string][] = [];
    const hooks = { openRunChanges: (paneId: string, runId: string) => opened.push([paneId, runId]) };
    const { calls, call } = recorder();
    expect(await runReviewAction(call, item({}), 'commitMerge', hooks)).toBe(true);
    expect(await runReviewAction(call, item({}), 'changes', hooks)).toBe(false);
    expect(await runReviewAction(call, item({}), 'pr', hooks)).toBe(true);
    expect(await runReviewAction(call, item({}), 'discard', hooks)).toBe(true);
    expect(await runReviewAction(call, item({}), 'editor', hooks)).toBe(false);
    expect(await runReviewAction(call, child, 'merge', hooks)).toBe(true);
    expect(await runReviewAction(call, child, 'discard', hooks)).toBe(true);
    expect(await runReviewAction(call, child, 'changes', hooks)).toBe(false);
    expect(await runReviewAction(call, child, 'editor', hooks)).toBe(false);
    expect(await runReviewAction(call, request, 'editor', hooks)).toBe(false);
    expect(await runReviewAction(call, request, 'changes', hooks)).toBe(false);
    expect(await runReviewAction(call, request, 'focus', hooks)).toBe(false);
    expect(calls).toEqual([
      ['paneWorktreeAction', { id: 'a', action: 'commitMerge' }],
      ['paneWorktreeAction', { id: 'a', action: 'changes' }],
      ['paneWorktreeAction', { id: 'a', action: 'pr' }],
      ['paneWorktreeAction', { id: 'a', action: 'discard' }],
      ['openInEditor', { source: 'worktree', id: 'a', path: '' }],
      ['childAction', { id: 'c', action: 'merge' }],
      ['childAction', { id: 'c', action: 'discard' }],
      ['reviewChanges', { id: 'c' }],
      ['openInEditor', { source: 'worktree', id: 'c', path: '' }],
      ['openInEditor', { source: 'reference', paneId: 'p', path: '' }],
      ['selectSession', { id: 'p' }],
    ]);
    // A request's changes open 요청 변경 검토 on the page.
    expect(opened).toEqual([['p', 'run-1']]);
  });

  it('keeps the queue reviewDismiss answers', async () => {
    const answer: ReviewQueue = { items: [], count: 0 };
    const { calls, call } = recorder(answer);
    setReviewQueue({ items: [request], count: 1 });
    await runReviewAction(call, request, 'dismiss', { openRunChanges: () => {} });
    expect(calls).toEqual([['reviewDismiss', { key: 'request:p:run-1' }]]);
  });
});

describe('the review panel\'s list (ReviewPanel.tsx)', () => {
  it('waits for the first reading, then says when there is nothing', () => {
    expect(render(createElement(ReviewList, { queue: null, t, working: null, onAction: () => {} }))).toContain('dialog-loading');
    expect(render(createElement(ReviewList, { queue: { items: [], count: 0 }, t, working: null, onAction: () => {} }))).toContain('Nothing to review.');
  });

  it('draws each item\'s pane, agent, kind, branch, size, state and actions', () => {
    const html = render(createElement(ReviewList, { queue: { items: [child, item({}), request, merged], count: 3 }, t, working: null, onAction: () => {} }));
    expect(html).toContain('Login fix');
    expect(html).toContain('Claude · Worktree pane · app');
    expect(html).toContain('Codex · Child pane · app');
    expect(html).toContain('mighty/a → main');
    expect(html).toContain('3 files');
    expect(html).toContain('+12');
    expect(html).toContain('−4');
    expect(html).toContain('data-status="conflict"');
    expect(html).toContain('Conflict');
    expect(html).toContain('fix the bug');
    expect(html).toContain('data-testid="review-commitMerge-worktree:a:ready:2:3:12:4"');
    expect(html).toContain('data-testid="review-pr-worktree:a:ready:2:3:12:4"');
    expect(html).not.toContain('data-testid="review-merge-request:p:run-1"');
    expect(html).toContain('Merged');
  });

  it('dims an item\'s git actions while one of them runs, leaving the editor and the pane', () => {
    const html = render(createElement(ReviewList, { queue: { items: [item({ busy: true })], count: 1 }, t, working: null, onAction: () => {} }));
    const button = (action: string) => new RegExp('<button[^>]*data-testid="review-' + action + '-[^"]*"[^>]*>').exec(html)?.[0] ?? '';
    expect(button('commitMerge')).toContain('disabled');
    expect(button('discard')).toContain('disabled');
    expect(button('editor')).not.toContain('disabled');
    expect(button('focus')).not.toContain('disabled');
  });
});

describe('opening the review queue', () => {
  const key = (code: string, mods: Partial<{ ctrlKey: boolean; shiftKey: boolean; altKey: boolean; metaKey: boolean }>) =>
    ({ key: code.replace('Key', '').toLowerCase(), code, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...mods });

  it('is Ctrl+Shift+R on Windows and ⇧⌘R on the Mac; plain Ctrl+R and ⌘R stay the page\'s', () => {
    expect(shortcutAction(key('KeyR', { ctrlKey: true, shiftKey: true }))).toBe('reviewQueue');
    expect(shortcutAction(key('KeyR', { ctrlKey: true }))).toBeNull();
    expect(macShortcutAction(key('KeyR', { metaKey: true, shiftKey: true }))).toBe('reviewQueue');
    expect(macShortcutAction(key('KeyR', { metaKey: true }))).toBeNull();
    expect(macShortcutAction(key('KeyR', { ctrlKey: true, shiftKey: true }))).toBeNull();
  });

  it('is an item of the Mac\'s menu bar', () => {
    expect(isMenuPush({ kind: 'menu', action: 'reviewQueue' } as never)).toBe(true);
  });
});

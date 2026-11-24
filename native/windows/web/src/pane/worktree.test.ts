import { describe, expect, it } from 'vitest';
import { diffLines, newSideLines, worktreeBar } from './worktree';
import type { PaneWorktree } from '../store/models';

const t = (key: string, vars?: Record<string, string | number>) => key + (vars ? JSON.stringify(vars) : '');
const pane: PaneWorktree = { worktreePath: 'C:\\wt\\p', branch: 'mighty/p', baseBranch: 'main', baseCommit: 'a'.repeat(40), checkout: 'C:\\repo', port: 4141 };

describe('the worktree bar', () => {
  it('merges when nothing is changed and offers commit and merge while files are', () => {
    expect(worktreeBar(pane, null, t).primary.action).toBe('merge');
    const changed = worktreeBar(pane, { ahead: 2, behind: 1, changed: 3, known: true, busy: false, conflict: false, canUndo: false }, t);
    expect(changed.primary.action).toBe('commitMerge');
    expect([changed.ahead?.text, changed.behind?.text, changed.changed?.text]).toEqual(['↑2', '↓1', '±3']);
  });

  it('asks to resolve after a conflict and offers undo after a merge', () => {
    const model = worktreeBar({ ...pane, setupStatus: 'failed' }, { ahead: 1, behind: 0, changed: 0, known: true, busy: false, conflict: true, canUndo: true }, t);
    expect(model.resolve?.action).toBe('resolve');
    expect(model.behind).toBeUndefined();
    expect(model.setup?.tone).toBe('err');
    expect(model.menu.map((m) => m.action)).toEqual(['changes', 'terminal', 'reveal', 'editor', 'port', 'undo', 'pr', 'discard']);
    const remote = worktreeBar(pane, { ahead: 0, behind: 0, changed: 0, known: true, busy: false, conflict: false, canUndo: false, remote: true }, t);
    expect(remote.menu.map((m) => m.action)).toEqual(['changes', 'terminal', 'port', 'pr', 'discard']);
  });

  it('reads a diff line by line', () => {
    expect(diffLines('--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n same').map((l) => l.kind)).toEqual(['meta', 'meta', 'hunk', 'del', 'add', 'same']);
  });

  it('numbers each diff line by the file as it is now, for 편집기에서 열기', () => {
    const diff = 'diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -10,4 +12,5 @@ fn\n keep\n-gone\n---dashes removed\n+new\n+more\n keep\n\\ No newline at end of file';
    expect(newSideLines(diff)).toEqual([null, null, null, 12, 12, 13, 13, 13, 14, 15, null]);
    expect(newSideLines('@@ -1 +0,0 @@\n-only')).toEqual([1, 1]);
    expect(newSideLines('')).toEqual([null]);
  });
});

describe('the worktree bar port block and run actions (MC-25)', () => {
  it('shows the block when one is assigned and each running action with its address', () => {
    const none = worktreeBar(pane, null, t);
    expect(none.portBlock).toBeUndefined();
    expect(none.actions).toEqual([]);
    const status = {
      ahead: 0, behind: 0, changed: 0, known: true, busy: false, conflict: false, canUndo: false,
      actions: [{ name: 'dev', url: 'http://localhost:41010/' }, { name: 'e2e', url: null }, { name: 'bad', url: 'javascript:alert(1)' }],
    };
    const model = worktreeBar({ ...pane, portBase: 41010, portCount: 10 }, status, t);
    expect(model.portBlock?.text).toBe(':41010–41019');
    expect(model.portBlock?.help).toContain('paneWorktree.bar.portBlockHelp');
    expect(model.actions.map((a) => [a.name, a.url])).toEqual([['dev', 'http://localhost:41010/'], ['e2e', null], ['bad', null]]);
    expect(model.actions[0]?.help).toContain('paneWorktree.bar.actionAt');
    expect(model.actions[1]?.help).toContain('paneWorktree.bar.actionRunning');
  });
});

// Pre-merge checks (MC-27; checksModel.ts, ChecksTab.tsx, src-tauri merge_checks.rs): the follow-up the chosen review threads become, the
// words of warnings and of parts that could not be read, the tab as drawn, and the review queue's summary that opens it.
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { ReviewRow } from '../review/ReviewPanel';
import { openChecks, type ReviewItem } from '../review/queue';
import tabSource from './ChecksTab.tsx?raw';
import modelSource from './checksModel.ts?raw';
import { ChecksView } from './ChecksTab';
import {
  FollowUpLimit, ciCounts, followUpHeader, threadPlace, threadsFollowUp, unknownReason, warningLines,
  type ChecksModel, type ChecksThread,
} from './checksModel';

const words: Record<string, string> = {
  'git.checks.followUp': 'PR #{number}: please address the review comments.',
  'git.checks.followUpMr': 'MR !{number}: please address the review comments.',
  'git.checks.unknown': "Can't check",
  'git.checks.reason.notLoggedIn': 'Not signed in to {cli}.',
  'git.checks.reason.failed': "Couldn't read it.",
  'git.pr.noRemote': 'No origin remote.',
  'git.pr.none': 'No pull request for this branch.',
  'git.pr.error.ghMissing': 'gh is missing.',
  'git.pr.error.glabMissing': 'glab is missing.',
  'git.pr.line': 'PR #{number} · {state}',
  'git.pr.state.open': 'open',
  'git.pr.open': 'Open',
  'git.pr.fixChecks': 'Ask to fix',
  'git.pr.checks.passed': 'Passed',
  'git.pr.checks.failed': 'Failed',
  'git.pr.checks.pending': 'Running',
  'git.checks.warn.uncommitted': '{count} uncommitted changes',
  'git.checks.warn.threads': '{count} unresolved review threads',
  'git.checks.warn.checksFailed': '{count} failed CI checks',
  'git.checks.warn.checklist': '{count} to-dos left',
  'git.checks.warn.draft': 'The PR is a draft',
  'git.checks.threads.toComposer': 'Put in Composer',
  'git.checks.threads.replies': '{count} replies',
  'git.checks.ci.summary': '{passed} passed · {failed} failed · {pending} running',
  'git.checks.checklist.noList': 'No request checklist',
  'review.checks.ok': 'Checks ✓',
  'review.checks.warn': 'Checks ⚠ {count}',
  'review.checks.help': 'Opens the Checks tab.',
};
const t = translator([words]);

const thread = (over: Partial<ChecksThread>): ChecksThread => ({
  id: 't1', path: 'src/a.ts', line: 120, outdated: false, author: 'reviewer', body: 'Null is not handled here.', url: 'https://github.com/o/r/pull/42#discussion_r1', replies: 0, ...over,
});

describe('the follow-up the chosen threads become', () => {
  it('lists them in file and line order under the header, in the line-note shape', () => {
    const text = threadsFollowUp(followUpHeader({ hosting: 'github', number: 42 }, t), [
      thread({ id: 'b', path: 'src/b.rs', line: 33, body: 'Rename the function.' }),
      thread({ id: 'a', body: 'Null is not handled here.\nSee the caller.' }),
      thread({ id: 'c', path: '', line: null, author: 'lead', body: 'Add a test.' }),
    ]);
    expect(text).toBe([
      'PR #42: please address the review comments.',
      '- (lead): Add a test.',
      '- src/a.ts:120 (reviewer): Null is not handled here.\n  See the caller.',
      '- src/b.rs:33 (reviewer): Rename the function.',
    ].join('\n'));
    expect(followUpHeader({ hosting: 'gitlab', number: 7 }, t)).toBe('MR !7: please address the review comments.');
  });

  it('makes nothing of no threads and caps the text', () => {
    expect(threadsFollowUp('header', [])).toBe('');
    const long = threadsFollowUp('header', [thread({ body: 'x'.repeat(5000) })]);
    expect([...long].length).toBe(FollowUpLimit);
    expect(long.endsWith('…')).toBe(true);
  });

  it('names a place as the agent opens it', () => {
    expect(threadPlace({ path: 'a.ts', line: 3 })).toBe('a.ts:3');
    expect(threadPlace({ path: 'a.ts', line: null })).toBe('a.ts');
    expect(threadPlace({ path: '', line: 3 })).toBe('');
  });
});

describe('the words of warnings and of parts that could not be read', () => {
  it('says why each part could not be read', () => {
    expect(unknownReason({ ok: false, reason: 'cliMissing', cli: 'glab' }, t)).toBe('glab is missing.');
    expect(unknownReason({ ok: false, reason: 'cliMissing', cli: 'gh' }, t)).toBe('gh is missing.');
    expect(unknownReason({ ok: false, reason: 'notLoggedIn', cli: 'gh' }, t)).toBe('Not signed in to gh.');
    expect(unknownReason({ ok: false, reason: 'noPr' }, t)).toBe('No pull request for this branch.');
    expect(unknownReason({ ok: false, reason: 'noRemote' }, t)).toBe('No origin remote.');
    expect(unknownReason({ ok: false, reason: 'whatever' }, t)).toBe("Couldn't read it.");
  });

  it('counts checks and words the warnings', () => {
    expect(ciCounts([{ name: 'a', state: 'passed', url: '' }, { name: 'b', state: 'failed', url: '' }, { name: 'c', state: 'skipped', url: '' }]))
      .toEqual({ passed: 1, failed: 1, pending: 0, skipped: 1 });
    expect(warningLines([{ code: 'checksFailed', count: 2 }, { code: 'draft', count: 1 }], t)).toEqual(['2 failed CI checks', 'The PR is a draft']);
  });
});

const model: ChecksModel = {
  git: { ok: true, value: { head: 'mighty/a', base: 'main', uncommitted: 2, conflicted: 0, ahead: 3, behind: 0 } },
  pr: { ok: true, value: { hosting: 'github', number: 42, url: 'https://github.com/o/r/pull/42', state: 'open', title: 'Fix login', isDraft: false, mergeable: 'mergeable', reviewDecision: '' } },
  checks: { ok: true, value: [{ name: 'CI / test', state: 'failed', url: 'https://github.com/o/r/runs/2' }] },
  threads: { ok: true, value: [thread({}), thread({ id: 't2', path: 'src/b.rs', line: 3, replies: 2 })] },
  checklist: { ok: false, reason: 'failed' },
  warnings: [{ code: 'uncommitted', count: 2 }, { code: 'checksFailed', count: 1 }, { code: 'threads', count: 2 }],
};

const services = { bridge: { call: () => new Promise(() => {}) }, store: { dispatch: () => {} }, t } as unknown as AppServices;
const render = (element: ReturnType<typeof createElement>) => renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, element));

describe('the Checks tab as drawn', () => {
  it('shows each part, a part it could not read on its own, the threads and their actions', () => {
    const html = render(createElement(ChecksView, {
      model, t, busy: false, selected: new Set(['t2']), setSelected: () => {}, open: () => {}, onFixChecks: () => {}, onToComposer: () => {},
    }));
    for (const id of ['git', 'pr', 'ci', 'threads', 'checklist']) expect(html).toContain('data-testid="git-checks-' + id + '"');
    expect(html).toMatch(/data-testid="git-checks-git" data-state="warn"/);
    expect(html).toMatch(/data-testid="git-checks-checklist" data-state="unknown"/);
    expect(html).toContain("Can&#x27;t check");
    expect(html).toContain('PR #42 · open');
    expect(html).toContain('data-testid="git-checks-fix"');
    expect(html).toContain('data-testid="git-checks-thread-t1"');
    expect(html).toContain('src/b.rs:3');
    expect(html).toContain('2 replies');
    expect(html).toMatch(/data-testid="git-checks-to-composer"(?![^>]*disabled)/);
  });

  it('keeps colour literals and Hangul out of its files', () => {
    for (const [name, source] of [['ChecksTab.tsx', tabSource], ['checksModel.ts', modelSource]] as const) {
      expect(source, name).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
      expect(source, name).not.toMatch(/\b(?:rgb|rgba|hsl|hsla)\s*\(/);
    }
    for (const source of [tabSource, modelSource]) expect(source).not.toMatch(/[\uAC00-\uD7A3]/);
  });
});

describe('the review queue summary', () => {
  const item: ReviewItem = {
    key: 'worktree:a:ready:2:3:12:4', kind: 'worktree', id: 'a', paneId: 'a', title: 'Login fix', agent: 'Claude', workspace: 'app', workspaceId: 'ws1',
    branch: 'mighty/a', base: 'main', status: 'ready', busy: false, files: 3, added: 12, removed: 4, canMerge: true, canCommitMerge: false, canPr: true,
    warnings: [{ code: 'checksFailed', count: 1 }],
  };

  it('shows ✓ or the warnings on the row, none when not read', () => {
    const warn = render(createElement(ReviewRow, { item, t, working: false, onAction: () => {} }));
    expect(warn).toContain('data-testid="review-checks-' + item.key + '"');
    expect(warn).toContain('Checks ⚠ 1');
    expect(warn).toContain('1 failed CI checks');
    expect(render(createElement(ReviewRow, { item: { ...item, warnings: [] }, t, working: false, onAction: () => {} }))).toContain('Checks ✓');
    expect(render(createElement(ReviewRow, { item: { ...item, warnings: undefined }, t, working: false, onAction: () => {} }))).not.toContain('review-checks-');
  });

  it('opens the Git workbench on its Checks tab for the pane', () => {
    const events: Event[] = [];
    openChecks(item, (event) => events.push(event));
    expect(events).toHaveLength(1);
    const [event] = events as CustomEvent[];
    expect(event?.type).toBe('mighty:open-git-workbench');
    expect(event?.detail).toEqual({ workspaceId: 'ws1', paneId: 'a', tab: 'checks' });
  });
});

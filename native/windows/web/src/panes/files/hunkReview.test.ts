// 바뀐 부분별 받기·버리기 (hunkReview.ts, DiffView's hunk heads, runChangesModel's progress): which hunk the keys act on, what each
// key does, the files' progress and 남은 부분 모두 받기, and the heads drawn with their buttons.
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../../app/context';
import { translator } from '../../i18n/locale';
import { reviewedCount, unreviewedPaths, type RunChangesItem } from '../../mighty/runChangesModel';
import { DiffView } from './DiffView';
import {
  canAct, firstOpen, hunkKey, keepCurrent, progressText, RefusalChip, RefusalText, reviewable, stepHunk, type HunkState,
} from './hunkReview';
import type { FileChanges } from './types';

const open = (fingerprint: string, extra: Partial<HunkState> = {}): HunkState => ({ fingerprint, ...extra });
const taken = (fingerprint: string): HunkState => ({ fingerprint, decision: 'accepted' });

describe('the current hunk', () => {
  it('starts at the first hunk not taken and moves with ] and [, staying at the ends', () => {
    expect(firstOpen([taken('a'), open('b'), open('c')])).toBe(1);
    expect(firstOpen([taken('a'), taken('b')])).toBe(0);
    expect(firstOpen([open('a'), taken('b'), open('c')], 1)).toBe(2);
    expect(firstOpen([open('a'), taken('b'), taken('c')], 2)).toBe(0);
    expect(stepHunk(3, 0, 1)).toBe(1);
    expect(stepHunk(3, 2, 1)).toBe(2);
    expect(stepHunk(3, 0, -1)).toBe(0);
    expect(stepHunk(0, 0, 1)).toBe(0);
  });

  it('stays on the same hunk across a new reading, else goes to the next open one', () => {
    const before = [open('a'), open('b'), open('c')];
    // b taken: the next open one after it.
    expect(keepCurrent(before, [open('a'), taken('b'), open('c')], 1)).toBe(2);
    // b put back (gone): the hunk now at its place.
    expect(keepCurrent(before, [open('a'), open('c')], 1)).toBe(1);
    // The last one put back: the one now last.
    expect(keepCurrent(before, [open('a'), open('b')], 2)).toBe(1);
    // Another hunk changed: still on c.
    expect(keepCurrent(before, [open('x'), open('b'), open('c')], 2)).toBe(2);
    expect(keepCurrent(before, [], 1)).toBe(0);
  });
});

describe('the keys', () => {
  const key = (k: string, extra: object = {}) => hunkKey({ key: k, ctrlKey: false, metaKey: false, altKey: false, target: null, ...extra });

  it('move with ] and [, take with a and put back with x', () => {
    expect([key(']'), key('['), key('a'), key('x'), key('A'), key('X'), key('b')]).toEqual(['next', 'previous', 'accept', 'discard', 'accept', 'discard', null]);
  });

  it('do nothing while typing or with a modifier held', () => {
    expect(key('a', { target: { tagName: 'INPUT' } })).toBeNull();
    expect(key('x', { target: { tagName: 'textarea' } })).toBeNull();
    expect(key('a', { target: { tagName: 'DIV', isContentEditable: true } })).toBeNull();
    expect(key('a', { metaKey: true })).toBeNull();
    expect(key(']', { ctrlKey: true })).toBeNull();
    expect(key('a', { target: { tagName: 'BUTTON' } })).toBe('accept');
    expect(key('x', { target: { tagName: 'INPUT', type: 'checkbox' } })).toBe('discard');
  });

  it('take a hunk once and never put back a refused one', () => {
    expect(canAct(open('a'), 'accept')).toBe(true);
    expect(canAct(taken('a'), 'accept')).toBe(false);
    expect(canAct(taken('a'), 'discard')).toBe(true);
    expect(canAct(open('a', { refusal: 'laterRun' }), 'discard')).toBe(false);
    expect(canAct(undefined, 'accept')).toBe(false);
  });
});

describe('progress', () => {
  it('reads n/m, nothing for a file with no hunks', () => {
    expect(progressText({ handled: 1, total: 3 })).toBe('1/3');
    expect(progressText({ handled: 0, total: 0 })).toBe('');
    expect(progressText(undefined)).toBe('');
  });

  it('counts the files reviewed and offers the rest to take', () => {
    const items: RunChangesItem[] = [
      { path: 'a', action: 'restore', restored: 1, removed: 1, hunks: 2, handled: 2, reviewed: true },
      { path: 'b', action: 'restore', restored: 1, removed: 1, hunks: 3, handled: 1, reviewed: false },
      { path: 'c', action: 'trash', restored: 0, removed: 4, hunks: 1, handled: 0, reviewed: false },
      { path: 'd', action: 'restore', restored: 0, removed: 0, refusal: 'unreadable', hunks: 0, handled: 0, reviewed: false },
    ];
    expect(reviewedCount(items)).toBe(1);
    expect(unreviewedPaths(items)).toEqual(['b', 'c']);
  });

  it('knows a review with hunks from one that cannot be reviewed', () => {
    const changes: FileChanges = { base: 'run', hunks: [{ oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: [{ op: '-', text: 'a' }, { op: '+', text: 'b' }] }], added: 1, removed: 1, truncated: false, unchanged: false };
    expect(reviewable({ changes, hunks: [open('a')], handled: 0, total: 1 })).toBe(true);
    expect(reviewable({ hunks: [], handled: 0, total: 0, refusal: 'missing' })).toBe(false);
    expect(reviewable({ changes: { ...changes, hunks: [], unchanged: true }, hunks: [], handled: 1, total: 1 })).toBe(false);
    expect(Object.keys(RefusalText)).toEqual(Object.keys(RefusalChip));
  });
});

describe('the hunk heads', () => {
  it('draw 받기 and 버리기 on each hunk, a taken one folded and a refused one without 버리기', () => {
    const t = translator([{ 'hunks.accept': 'Accept ✓', 'hunks.discard': 'Discard ↶', 'hunks.accepted': 'Accepted', 'hunks.refusalShort.laterRun': 'Changed later' }]);
    const services = { bridge: { call: () => Promise.resolve(null) }, store: {}, t } as unknown as AppServices;
    const hunk = (start: number) => ({ oldStart: start, oldLines: 1, newStart: start, newLines: 1, lines: [{ op: '-' as const, text: 'a' }, { op: '+' as const, text: 'b' }] });
    const changes: FileChanges = { base: 'run', hunks: [hunk(1), hunk(20)], added: 2, removed: 2, truncated: false, unchanged: false };
    const controls = { states: [taken('a'), { ...taken('b'), refusal: 'laterRun' as const }], current: 1, busy: false, onCurrent: () => {}, onAct: () => {} };
    // The theme is read off <html> (useAppTheme); the taken hunks are folded, so nothing needs the renderer's browser.
    const page = globalThis as { document?: unknown };
    const had = page.document;
    page.document = { documentElement: { dataset: {} } };
    let html = '';
    try {
      html = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(DiffView, { changes, name: 'a.ts', hunks: controls })));
    } finally {
      page.document = had;
    }
    expect(html).toContain('@@ -1,1 +1,1 @@');
    expect(html).toContain('@@ -20,1 +20,1 @@');
    expect(html.match(/Accept ✓/g)?.length).toBe(2);
    expect(html).toContain('Changed later');
    expect(html).toMatch(/files-diff-hunk handled current/);
    expect(html).toMatch(/data-testid="hunk-discard-1"[^>]*disabled/);
    expect(html).toMatch(/data-testid="hunk-accept-0"[^>]*disabled/);
  });
});

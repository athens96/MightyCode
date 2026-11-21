// 요청 변경 검토 (RunChanges.tsx, runChangesModel.ts; Swift ReviewNotesTests and the sheet's rules): the notes' follow-up request,
// which lines carry a note, which results offer the review, and the panel's first draw.
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import type { MightyGraphRun } from '../store/models';
import { emptyState, reduce } from '../store/store';
import { diffRows, placeOf, rowOf } from '../panes/files/diff';
import type { FileChanges } from '../panes/files/types';
import { Block, type BlockActions } from './Block';
import { blockExtras } from './extras';
import type { Layout } from './layout';
import { RunChangesDialog } from './RunChanges';
import {
  MaximumQuote, appendingBlock, noteTarget, notePlace, notedRows, revertible, reviewPrompt, settledRunsKey, type ReviewNote,
} from './runChangesModel';

const note = (path: string, line: number, side: 'old' | 'new', quote: string, text: string, id = 0): ReviewNote => ({ id, path, line, side, quote, text });

describe('the review notes as a follow-up request', () => {
  it('sorts the notes by place and names the line the agent can open', () => {
    const prompt = reviewPrompt([
      note('b.swift', 3, 'new', 'let x = 1', 'rename x'),
      note('a.swift', 9, 'old', '  guard ok else { return }  ', 'keep this guard'),
      note('a.swift', 2, 'new', '', '  why?\nexplain  '),
      note('c.swift', 1, 'new', 'x', '   '),
    ], 'Review notes:', 'removed line {line}');
    expect(prompt).toBe([
      'Review notes:',
      '- `a.swift:2`',
      '  why?',
      '  explain',
      '- `a.swift` (removed line 9) — `guard ok else { return }`',
      '  keep this guard',
      '- `b.swift:3` — `let x = 1`',
      '  rename x',
    ].join('\n'));
  });

  it('makes no request of no notes, and the block follows the draft', () => {
    expect(reviewPrompt([], 'h', '{line}')).toBe('');
    expect(appendingBlock('notes', '  ')).toBe('notes');
    expect(appendingBlock('notes', 'fix it\n')).toBe('fix it\n\nnotes');
    expect(appendingBlock('', 'draft')).toBe('draft');
  });

  it('cuts a long quote and keeps backticks from breaking the code span', () => {
    const long = 'a'.repeat(200);
    expect(reviewPrompt([note('f', 1, 'new', long + '`', 'n')], 'h', '{line}')).toContain('a'.repeat(MaximumQuote) + '…`');
    expect(reviewPrompt([note('f', 1, 'new', 'a `b` c', 'n')], 'h', '{line}')).toContain("`a 'b' c`");
  });

  it('puts the notes after the composer\'s words through a fill that appends', () => {
    const state = reduce(emptyState, { kind: 'fill', paneId: 'p', text: 'notes', append: true });
    expect(state.fills.p).toEqual({ text: 'notes', seq: 1, append: true });
    expect(reduce(state, { kind: 'fill', paneId: 'p', text: 'card' }).fills.p).toEqual({ text: 'card', seq: 2 });
  });
});

const changes: FileChanges = {
  base: 'run', added: 2, removed: 1, truncated: false, unchanged: false, runId: 'r1',
  hunks: [{ oldStart: 1, oldLines: 2, newStart: 1, newLines: 3, lines: [{ op: ' ', text: 'one' }, { op: '-', text: 'two' }, { op: '+', text: 'TWO' }, { op: '+', text: 'three' }] }],
};

describe('a note on a diff line', () => {
  const rows = diffRows(changes);

  it('sits on a kept or added line by its number now, on a removed one by its number before, never on a hunk head', () => {
    expect(rows.map(noteTarget)).toEqual([null, { line: 1, side: 'new' }, { line: 2, side: 'old' }, { line: 2, side: 'new' }, { line: 3, side: 'new' }]);
    expect(notePlace('src/a.ts', { line: 2, side: 'old' })).toBe('src/a.ts −2');
    expect(notePlace('src/a.ts', { line: 3, side: 'new' })).toBe('src/a.ts:3');
  });

  it('marks the rows of the shown file that carry one', () => {
    const notes = [note('a.ts', 2, 'old', 'two', 'why'), note('a.ts', 3, 'new', 'three', 'ok'), note('b.ts', 1, 'new', 'one', 'other file')];
    expect([...notedRows(rows, notes, 'a.ts')]).toEqual([2, 4]);
  });

  it('places the noted and the picked rows on the side the diff renderer draws them', () => {
    // DiffView hands these places to @pierre/diffs as annotations and the selected line (it draws in a browser only).
    expect(placeOf(rows[2]!)).toEqual({ side: 'deletions', lineNumber: 2 });
    expect(placeOf(rows[4]!)).toEqual({ side: 'additions', lineNumber: 3 });
    expect(rowOf(rows, 'deletions', 2)).toBe(2);
    expect(rowOf(rows, 'additions', 3)).toBe(4);
  });
});

describe('the results that offer the review', () => {
  const run = (id: string, status: string) => ({ id, status }) as MightyGraphRun;

  it('asks the host about the settled runs only, as one key', () => {
    expect(settledRunsKey([run('a', 'completed'), run('b', 'running'), run('c', 'failed'), run('d', 'stopped'), run('e', 'pending')])).toBe('a\nc\nd');
  });

  it('marks the result of a run the host named, and no other', () => {
    const runs = [{ ...run('r1', 'completed'), input: 'q', rootEntries: [], agents: [], resultEntries: [] }, { ...run('r2', 'completed'), input: 'q', rootEntries: [], agents: [], resultEntries: [] }] as MightyGraphRun[];
    const layout = { nodes: [{ id: '2:r1:result', kind: 'result', frame: { x: 0, y: 0, w: 500, h: 200 } }, { id: '2:r2:result', kind: 'result', frame: { x: 0, y: 0, w: 500, h: 200 } }], fittedResultID: null } as unknown as Layout;
    const base = { layout, runs, inputs: null, draft: '', planHistory: undefined, resultSize: undefined, planText: undefined, latest: null };
    const first = blockExtras({ ...base, changedRuns: new Set(['r2']) }, new Map());
    expect(first.get('2:r1:result')?.changes).toBeUndefined();
    expect(first.get('2:r2:result')?.changes).toBe(true);
    // The same answer again keeps the blocks' parts: nothing draws again.
    const again = blockExtras({ ...base, changedRuns: new Set(['r2']) }, first);
    expect(again.get('2:r2:result')).toBe(first.get('2:r2:result'));
  });

  it('puts the button on the result header, which opens the review of its run', () => {
    const t = translator([{ 'runChanges.openButton': 'Review this request\'s changes' }]);
    const services = { bridge: { call: () => Promise.resolve(null) }, store: {}, t } as unknown as AppServices;
    const props = { paneId: 'p', provider: 'claude', id: '2:r1:result', kind: 'result', x: 0, y: 0, w: 500, h: 200, copy: undefined,
      visible: false, selected: false, expanded: false, running: false, actions: {} as BlockActions };
    const draw = (extra: object) => renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(Block, { ...props, extra })));
    expect(draw({ changes: true })).toContain('data-testid="mighty-run-changes-2:r1:result" title="Review this request&#x27;s changes"');
    expect(draw({})).not.toContain('mighty-run-changes');
  });
});

describe('the review panel', () => {
  it('opens as a panel titled for the request, reading its files', () => {
    const t = translator([{ 'runChanges.title': 'Review request changes', 'runChanges.notes.send': 'Add {count} notes to the composer', 'runChanges.revertChosen': 'Revert {count} chosen' }]);
    const services = { bridge: { call: () => new Promise(() => {}) }, store: { dispatch: () => {} }, t } as unknown as AppServices;
    const html = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(RunChangesDialog, { paneId: 'p', runId: 'r1', onClose: () => {} })));
    expect(html).toContain('aria-label="Review request changes"');
    expect(html).toContain('data-testid="dialog-loading"');
    expect(html).toContain('Revert 0 chosen');
    expect(html).toContain('Add 0 notes to the composer');
  });

  it('chooses only the files that can still be put back', () => {
    expect(revertible([
      { path: 'a', action: 'restore', restored: 1, removed: 0 },
      { path: 'b', action: 'restore', restored: 1, removed: 0, refusal: 'laterRun' },
      { path: 'c', action: 'trash', restored: 0, removed: 3 },
    ]).map((item) => item.path)).toEqual(['a', 'c']);
  });
});

import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { hexAlpha, tokenStyleSheet } from '../theme/tokens';
import { Tree } from './files/FilesView';
import { fileRowId, lineNumbers, scrollRowIntoView, sourceSpans, treeKey, visibleTreeSlice } from './files/tree';
import type { FileRow, FilesModel } from './files/types';
import { paneKindWords, slimKind } from './Panes';
import { base64Bytes, clipboardKey, defaultFamily, terminalFamily, terminalSize } from './terminal/rules';

const row = (path: string, depth: number, dir = false, expanded = false): FileRow => ({ path, name: path.split('/').pop() ?? path, depth, dir, expanded, noise: false, symlink: false });
const rows = [row('src', 0, true, true), row('src/a.ts', 1), row('src/b.ts', 1), row('README.md', 0), row('docs', 0, true)];

describe('files tree keys', () => {
  it('moves the choice up and down, starting at the first row', () => {
    expect(treeKey(rows, undefined, 'ArrowDown', false)).toEqual({ kind: 'select', path: 'src' });
    expect(treeKey(rows, 'src/a.ts', 'ArrowDown', false)).toEqual({ kind: 'select', path: 'src/b.ts' });
    expect(treeKey(rows, 'src/a.ts', 'ArrowUp', false)).toEqual({ kind: 'select', path: 'src' });
    expect(treeKey(rows, 'docs', 'ArrowDown', false)).toEqual({ kind: 'none' });
  });

  it('opens a closed folder, goes into an open one, closes and goes up', () => {
    expect(treeKey(rows, 'docs', 'ArrowRight', false)).toEqual({ kind: 'toggle', path: 'docs' });
    expect(treeKey(rows, 'src', 'ArrowRight', false)).toEqual({ kind: 'select', path: 'src/a.ts' });
    expect(treeKey(rows, 'src', 'ArrowLeft', false)).toEqual({ kind: 'toggle', path: 'src' });
    expect(treeKey(rows, 'src/b.ts', 'ArrowLeft', false)).toEqual({ kind: 'select', path: 'src' });
    expect(treeKey(rows, 'README.md', 'ArrowLeft', false)).toEqual({ kind: 'none' });
    expect(treeKey(rows, 'docs', 'Enter', false)).toEqual({ kind: 'toggle', path: 'docs' });
    expect(treeKey(rows, 'README.md', 'Enter', false)).toEqual({ kind: 'none' });
  });

  it('reveals a folder from the filter results and ignores left there', () => {
    expect(treeKey(rows, 'docs', 'ArrowRight', true)).toEqual({ kind: 'reveal', path: 'docs' });
    expect(treeKey(rows, 'docs', 'Enter', true)).toEqual({ kind: 'reveal', path: 'docs' });
    expect(treeKey(rows, 'src/a.ts', 'ArrowLeft', true)).toEqual({ kind: 'none' });
  });
});

describe('files source view', () => {
  it('cuts the text at the coloured spans', () => {
    expect(sourceSpans('const a = "x"; // n', [0, 0, 5, 1, 10, 3, 2, 15, 4])).toEqual([
      { text: 'const', kind: 'keyword' }, { text: ' a = ' }, { text: '"x"', kind: 'string' }, { text: '; ' }, { text: '// n', kind: 'comment' },
    ]);
  });

  it('skips spans out of order or past the end', () => {
    expect(sourceSpans('abc', [0, 1, 1, 0, 0, 1, 3, 2, 5])).toEqual([{ text: 'a' }, { text: 'b', kind: 'keyword' }, { text: 'c' }]);
    expect(sourceSpans('plain', undefined)).toEqual([{ text: 'plain' }]);
  });

  it('numbers every line, at least one', () => {
    expect(lineNumbers(3)).toBe('1\n2\n3');
    expect(lineNumbers(0)).toBe('1');
  });
});

describe('terminal rules', () => {
  const key = (code: string, extra: Partial<{ shiftKey: boolean; altKey: boolean; isComposing: boolean; type: string; ctrlKey: boolean }> = {}) =>
    ({ type: 'keydown', code, ctrlKey: true, shiftKey: false, altKey: false, isComposing: false, ...extra });

  it('copies with a selection or Shift, pastes, and leaves Ctrl+C to the shell otherwise', () => {
    expect(clipboardKey(key('KeyC'), true)).toBe('copy');
    expect(clipboardKey(key('KeyC'), false)).toBeNull();
    expect(clipboardKey(key('KeyC', { shiftKey: true }), false)).toBe('copy');
    expect(clipboardKey(key('KeyV'), false)).toBe('paste');
    expect(clipboardKey(key('KeyV', { isComposing: true }), false)).toBeNull();
    expect(clipboardKey(key('KeyV', { altKey: true }), false)).toBeNull();
    expect(clipboardKey(key('KeyV', { type: 'keyup' }), false)).toBeNull();
  });

  it('reads base64 output as UTF-8 bytes', () => {
    expect(new TextDecoder().decode(base64Bytes(btoa(String.fromCharCode(...new TextEncoder().encode('ok 한글')))))).toBe('ok 한글');
  });

  it('takes the code font first and a size in range', () => {
    expect(terminalFamily('Fira "Code"')).toBe('"Fira Code", ' + defaultFamily);
    expect(terminalFamily('')).toBe(defaultFamily);
    expect(terminalSize(13)).toBe(13);
    expect(terminalSize(80)).toBeNull();
  });
});

describe('slim panes', () => {
  const t = (key: string) => key;
  it('stands over every kind but an agent and a shell, a kind it does not know too', () => {
    for (const kind of ['agent-terminal', 'files', 'workmap']) expect(slimKind(kind)).toBe(true);
    expect(slimKind('claude')).toBe(false);
    expect(slimKind('shell')).toBe(false);
    // An older computer's browser pane (a remote mirror): its kind's name over an empty body.
    expect(slimKind('browser')).toBe(true);
    expect(paneKindWords('browser', t)).toBe('browser');
  });

  it('names the kinds the slim bar stands over', () => {
    expect(paneKindWords('agent-terminal', t)).toBe('dashboard.kind.agentTerminal');
    expect(paneKindWords('files', t)).toBe('files.pane.title');
    expect(paneKindWords('workmap', t)).toBe('workMap.title');
  });

  it('has the subtle wash, the syntax colours and the layout metrics as variables', () => {
    expect(hexAlpha('#FFFFFF', 0.035)).toBe('rgba(255,255,255,0.035)');
    const sheet = tokenStyleSheet();
    expect(sheet).toContain('--subtle:rgba(');
    expect(sheet).toContain('--syntax-keyword:');
    expect(sheet).toContain('--layout-pane-header:');
  });
});

describe('files tree row IDs & virtualization', () => {
  const services = {
    bridge: { call: () => Promise.resolve(null) },
    store: { dispatch: () => {} },
    t: translator([{}]),
  } as unknown as AppServices;

  const renderTree = (model: FilesModel) =>
    renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(Tree, { model, onRow: () => {}, onKey: () => {} }),
      ),
    );

  it('URI-encodes file row IDs and aria-activedescendant so spaces and special characters form valid HTML IDREFs', () => {
    const specialPath = 'src/my folder/file #1 [draft].ts';
    expect(fileRowId(specialPath)).toBe('files-row-' + encodeURIComponent(specialPath));
    expect(fileRowId(specialPath)).not.toMatch(/[\s#[\]]/);

    const model: FilesModel = {
      available: true,
      filter: '',
      filtering: false,
      selected: specialPath,
      rows: [row('src/my folder', 0, true, true), row(specialPath, 1)],
      notes: [],
    };
    const html = renderTree(model);
    const encoded = 'files-row-' + encodeURIComponent(specialPath);
    expect(html).toContain(`aria-activedescendant="${encoded}"`);
    expect(html).toContain(`id="${encoded}"`);
    expect(html).toContain(`data-path="${specialPath}"`);
  });

  it('computes visibleTreeSlice window bounds and invariant spacer heights', () => {
    expect(visibleTreeSlice(0, 0, 440, 22, 10)).toEqual({ start: 0, end: 0, topSpacer: 0, bottomSpacer: 0 });

    const top = visibleTreeSlice(1000, 0, 440, 22, 10);
    expect(top).toEqual({ start: 0, end: 30, topSpacer: 0, bottomSpacer: (1000 - 30) * 22 });

    const mid = visibleTreeSlice(1000, 2200, 440, 22, 10);
    expect(mid).toEqual({ start: 90, end: 130, topSpacer: 90 * 22, bottomSpacer: (1000 - 130) * 22 });
    expect(mid.topSpacer + (mid.end - mid.start) * 22 + mid.bottomSpacer).toBe(1000 * 22);

    const end = visibleTreeSlice(1000, 999_999, 440, 22, 10);
    expect(end.end).toBe(1000);
    expect(end.bottomSpacer).toBe(0);
    expect(end.end - end.start).toBe(30);
    expect(end.topSpacer + (end.end - end.start) * 22 + end.bottomSpacer).toBe(1000 * 22);

    expect(scrollRowIntoView(5, 22, 100, 200)).toBeNull();
    expect(scrollRowIntoView(2, 22, 100, 200)).toBe(44);
    expect(scrollRowIntoView(20, 22, 100, 200)).toBe(21 * 22 - 200);
  });

  it('renders all rows directly when rows.length <= 200 and windows when rows.length > 200', () => {
    const makeRows = (count: number) => Array.from({ length: count }, (_, i) => row(`src/file-${i}.ts`, 1));

    const smallHtml = renderTree({
      available: true,
      filter: '',
      filtering: false,
      selected: 'src/file-0.ts',
      rows: makeRows(200),
      notes: [],
    });
    expect(smallHtml.match(/role="treeitem"/g)).toHaveLength(200);

    const largeHtml = renderTree({
      available: true,
      filter: '',
      filtering: false,
      selected: 'src/file-0.ts',
      rows: makeRows(500),
      notes: [],
    });
    const renderedCount = largeHtml.match(/role="treeitem"/g)?.length ?? 0;
    expect(renderedCount).toBeGreaterThan(20);
    expect(renderedCount).toBeLessThan(80);
    expect(largeHtml).toContain(`height:${(500 - renderedCount) * 22}px`);
  });
});

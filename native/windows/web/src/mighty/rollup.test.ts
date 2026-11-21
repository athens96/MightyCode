import { describe, expect, it } from 'vitest';
import type { MightyGraphRun, RunCheckpoint } from '../store/models';
import { byFile, foldered, followed, matches, openAtFirst, requestOf, rollUp } from './rollup';

const tree = (c: string) => c.repeat(40);

function run(id: string, checkpoints: RunCheckpoint[], status = 'completed'): MightyGraphRun {
  return { id, input: '', status, rootEntries: [], agents: [], resultEntries: [], checkpoints, ...(id.includes(':notice:') ? { noticeRun: true } : {}) };
}

const start: RunCheckpoint = { kind: 'start', tree: tree('0'), files: [] };
const parser: RunCheckpoint = {
  kind: 'step', label: '파서 작성', stepKey: 'id:1', tree: tree('1'), base: tree('0'),
  files: [{ path: 'src/parse.rs', status: 'A', added: 40, removed: 0 }, { path: 'src/lib.rs', status: 'M', added: 2, removed: 1 }],
};
const tests: RunCheckpoint = {
  kind: 'step', label: '테스트', stepKey: 'id:2', tree: tree('2'), base: tree('1'),
  files: [{ path: 'src/parse.rs', status: 'M', added: 3, removed: 1 }, { path: 'tests/parse.rs', status: 'A', added: 20, removed: 0 }],
};
const build: RunCheckpoint = { kind: 'turn', label: 'build', tree: tree('3'), base: tree('2'), files: [{ path: 'dist/app.js', status: 'M' }] };

describe('a request\'s 결과물', () => {
  it('gathers the request\'s runs and its notice runs on the last result only', () => {
    const runs = [run('r', [start, parser, tests]), run('r:notice:1', [build]), run('other', [parser])];
    expect(requestOf('r:notice:1')).toBe('r');
    expect(followed(runs[0]!, runs)).toBe(true);
    expect(rollUp(runs, runs[0]!)).toEqual([]);
    const sections = rollUp(runs, runs[1]!);
    expect(sections.map((s) => [s.kind, s.label, s.base, s.tree])).toEqual([
      ['step', '파서 작성', tree('0'), tree('1')],
      ['step', '테스트', tree('1'), tree('2')],
      ['turn', 'build', tree('2'), tree('3')],
    ]);
    expect(sections[2]!.runId).toBe('r:notice:1');
  });

  it('lists each file once by file, with the steps that changed it and its lines summed', () => {
    const sections = rollUp([run('r', [start, parser, tests])], run('r', [start, parser, tests]));
    const files = byFile(sections);
    expect(files.map((f) => [f.path, f.status, f.added, f.removed, f.sections])).toEqual([
      ['src/parse.rs', 'A', 43, 1, [0, 1]],
      ['src/lib.rs', 'M', 2, 1, [0]],
      ['tests/parse.rs', 'A', 20, 0, [1]],
    ]);
  });

  it('follows a rename and keeps a deletion', () => {
    const renamed: RunCheckpoint = { kind: 'step', tree: tree('4'), base: tree('1'), files: [{ path: 'src/lib2.rs', oldPath: 'src/lib.rs', status: 'R', added: 0, removed: 0 }] };
    const gone: RunCheckpoint = { kind: 'end', tree: tree('5'), base: tree('4'), files: [{ path: 'src/lib2.rs', status: 'D', added: 0, removed: 3 }] };
    const files = byFile(rollUp([run('r', [parser, renamed, gone])], run('r', [parser, renamed, gone])));
    expect(files.find((f) => f.path === 'src/lib2.rs')).toMatchObject({ status: 'D', sections: [0, 1, 2] });
    expect(files.some((f) => f.path === 'src/lib.rs')).toBe(false);
  });

  it('puts three or more files of a folder under its heading', () => {
    const rows = foldered([{ path: 'a/x.ts' }, { path: 'a/y.ts' }, { path: 'a/z.ts' }, { path: 'b/w.ts' }, { path: 'README.md' }]);
    expect(rows.map((row) => ('folder' in row ? 'folder:' + row.folder : row.file.path))).toEqual(['folder:a', 'a/x.ts', 'a/y.ts', 'a/z.ts', 'b/w.ts', 'README.md']);
  });

  it('opens every section when there are few and the last two when there are many', () => {
    expect([...openAtFirst(3)]).toEqual([0, 1, 2]);
    expect([...openAtFirst(9)]).toEqual([7, 8]);
  });

  it('matches every word of a search, case aside', () => {
    expect(matches('src/Parse/Lexer.rs', 'parse lex')).toBe(true);
    expect(matches('src/Parse/Lexer.rs', 'parse test')).toBe(false);
  });
});

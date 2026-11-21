import { describe, expect, it } from 'vitest';
import type { RunSession } from '../../store/models';
import {
  computeBufferChanges,
  findMatches,
  formatAgentSelectionBlock,
  formatSelectionReference,
  indentSelection,
  newlineWithIndent,
  pickTargetAgentSession,
  renderEditorBackdropSegments,
  replaceAllMatches,
  replaceSingleMatch,
  selectionLineSpan,
} from './editorModel';

describe('files editorModel (MC-6)', () => {
  it('computes 1-based selection line spans and formats path:start-end context blocks', () => {
    const text = 'line 1\nconst x = 10;\nconst y = 20;\nreturn x + y;\n';
    const start = text.indexOf('const x');
    const end = text.indexOf('return');
    const span = selectionLineSpan(text, start, end);
    expect(span).toEqual({
      start,
      end,
      startLine: 2,
      endLine: 3,
      selectedText: 'const x = 10;\nconst y = 20;\n',
    });
    expect(formatSelectionReference('src\\panes\\FilesView.tsx', 2, 3)).toBe('src/panes/FilesView.tsx:2-3');
    expect(formatSelectionReference('src/app.ts', 7, 7)).toBe('src/app.ts:7');

    const block = formatAgentSelectionBlock('src/app.ts', 2, 3, span!.selectedText, '이 코드를 설명해줘:');
    expect(block).toBe(
      '이 코드를 설명해줘: `src/app.ts:2-3`\n```ts\nconst x = 10;\nconst y = 20;\n```\n',
    );
  });

  it('finds and replaces matches case-sensitively and case-insensitively', () => {
    const text = 'Alpha beta\nalpha BETA\ngamma';
    expect(findMatches(text, 'alpha', false)).toEqual([
      { start: 0, end: 5, line: 1 },
      { start: 11, end: 16, line: 2 },
    ]);
    expect(findMatches(text, 'alpha', true)).toEqual([{ start: 11, end: 16, line: 2 }]);

    const single = replaceSingleMatch(text, { start: 11, end: 16, line: 2 }, 'omega');
    expect(single.text).toBe('Alpha beta\nomega BETA\ngamma');
    expect(single.cursor).toBe(16);

    const all = replaceAllMatches(text, 'beta', 'delta', false);
    expect(all).toEqual({ text: 'Alpha delta\nalpha delta\ngamma', count: 2 });
  });

  it('indents, outdents and preserves leading whitespace on newline', () => {
    const src = 'fn main() {\n  let a = 1;\n  let b = 2;\n}\n';
    const selStart = src.indexOf('let a');
    const selEnd = src.indexOf('let b') + 5;
    const indented = indentSelection(src, selStart, selEnd, false);
    expect(indented.text).toBe('fn main() {\n    let a = 1;\n    let b = 2;\n}\n');

    const outdented = indentSelection(indented.text, indented.start, indented.end, true);
    expect(outdented.text).toBe(src);

    const nl = newlineWithIndent(src, src.indexOf('let a = 1;') + 10, src.indexOf('let a = 1;') + 10);
    expect(nl.text).toContain('  let a = 1;\n  \n  let b = 2;');
  });

  it('layers search highlights cleanly over syntax tokens', () => {
    const code = 'const answer = 42; // answer note';
    const matches = findMatches(code, 'answer', true);
    const segments = renderEditorBackdropSegments(code, 'src/demo.ts', matches, 1);
    expect(segments.map((s) => s.text).join('')).toBe(code);
    expect(segments.some((s) => s.token === 'keyword' && s.text === 'const')).toBe(true);
    expect(segments.some((s) => s.token === 'number' && s.text === '42')).toBe(true);
    expect(segments.filter((s) => s.match === 'match')).toHaveLength(1);
    expect(segments.filter((s) => s.match === 'current')).toHaveLength(1);
  });

  it('computes unified hunks between base and edited draft for instant Diff toggle', () => {
    const base = 'line 1\nline 2\nline 3\nline 4\n';
    const draft = 'line 1\nline two\nline 3\nline 4\nline 5\n';
    const changes = computeBufferChanges(base, draft, 'abc1234');
    expect(changes.unchanged).toBe(false);
    expect(changes.added).toBe(2);
    expect(changes.removed).toBe(1);
    expect(changes.hunks).toHaveLength(1);
    expect(computeBufferChanges(base, base).unchanged).toBe(true);
  });

  it('picks an agent conversation session in the same workspace, skipping files and shell panes', () => {
    const make = (id: string, kind: string, workspaceId: string, status: RunSession['status'] = 'idle'): RunSession =>
      ({ id, kind, workspaceId, status, title: id, provider: 'claude' } as unknown as RunSession);
    const sessions = [
      make('files:ws1', 'files', 'ws1'),
      make('shell:ws1', 'shell', 'ws1'),
      make('claude-1', 'claude', 'ws1', 'running'),
      make('codex-1', 'codex', 'ws1', 'idle'),
      make('claude-ws2', 'claude', 'ws2', 'idle'),
    ];
    expect(pickTargetAgentSession(sessions, 'ws1', 'files:ws1')?.id).toBe('codex-1');
    expect(pickTargetAgentSession(sessions, 'ws1', 'claude-1')?.id).toBe('claude-1');
  });
});

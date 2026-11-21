import type { RunSession } from '../../store/models';
import type { FileChanges, FileDiffHunk, FileDiffLine } from './types';

export interface CodeSelection {
  start: number;
  end: number;
  startLine: number;
  endLine: number;
  selectedText: string;
}

export interface SearchMatch {
  start: number;
  end: number;
  line: number;
}

export type HighlightKind = 'plain' | 'keyword' | 'string' | 'comment' | 'number';

export interface BackdropSegment {
  text: string;
  token: HighlightKind;
  match: 'none' | 'match' | 'current';
}

const MAXIMUM_MATCHES = 5_000;
const MAXIMUM_DIFF_LINES = 5_000;

/** Computes 1-based inclusive `[startLine, endLine]` and selected slice from UTF-16 editor offsets. */
export function selectionLineSpan(text: string, selectionStart: number, selectionEnd: number): CodeSelection | null {
  const from = Math.max(0, Math.min(selectionStart, selectionEnd, text.length));
  const to = Math.max(0, Math.min(Math.max(selectionStart, selectionEnd), text.length));
  if (from === to) return null;
  const selectedText = text.slice(from, to);
  let startLine = 1;
  for (let i = 0; i < from; i++) {
    if (text.charCodeAt(i) === 10) startLine++;
  }
  let endLine = startLine;
  const scanEnd = to > from && text.charCodeAt(to - 1) === 10 ? to - 1 : to;
  for (let i = from; i < scanEnd; i++) {
    if (text.charCodeAt(i) === 10) endLine++;
  }
  return { start: from, end: to, startLine, endLine, selectedText };
}

/** Resolves a DOM text selection inside a read-only `SourceView` back to 1-based line numbers in `fullText`. */
export function resolveDomCodeSelection(root: HTMLElement | null, fullText: string): CodeSelection | null {
  if (!root || typeof window === 'undefined') return null;
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
  const range = sel.getRangeAt(0);
  if (!root.contains(range.commonAncestorContainer)) return null;
  const raw = sel.toString().replace(/\r\n/g, '\n');
  if (raw.trim().length === 0) return null;
  const index = fullText.indexOf(raw);
  if (index >= 0) return selectionLineSpan(fullText, index, index + raw.length);
  const trimmed = raw.trim();
  const trimmedIndex = fullText.indexOf(trimmed);
  if (trimmedIndex >= 0) return selectionLineSpan(fullText, trimmedIndex, trimmedIndex + trimmed.length);
  const lines = raw.split('\n');
  return {
    start: 0,
    end: raw.length,
    startLine: 1,
    endLine: Math.max(1, lines.length),
    selectedText: raw,
  };
}

/** Formats `path:line` or `path:start-end`. */
export function formatSelectionReference(path: string, startLine: number, endLine: number): string {
  const clean = path.replace(/\\/g, '/');
  return startLine >= endLine ? `${clean}:${startLine}` : `${clean}:${startLine}-${endLine}`;
}

export function fenceLanguageForPath(path: string): string {
  const name = path.split('/').pop()?.toLowerCase() ?? '';
  if (name === 'dockerfile') return 'dockerfile';
  if (name === 'makefile') return 'makefile';
  const ext = name.includes('.') ? name.split('.').pop() ?? '' : '';
  const map: Record<string, string> = {
    ts: 'ts', tsx: 'tsx', mts: 'ts', cts: 'ts',
    js: 'js', jsx: 'jsx', mjs: 'js', cjs: 'js',
    rs: 'rust', py: 'python', kt: 'kotlin', kts: 'kotlin',
    swift: 'swift', cs: 'csharp', go: 'go', java: 'java',
    c: 'c', h: 'c', cpp: 'cpp', cc: 'cpp', hpp: 'cpp',
    json: 'json', yaml: 'yaml', yml: 'yaml', toml: 'toml',
    md: 'markdown', html: 'html', htm: 'html', css: 'css',
    sh: 'bash', zsh: 'zsh', sql: 'sql', xml: 'xml', svg: 'xml',
  };
  return map[ext] ?? '';
}

/** Formats a code selection as a Markdown reference + fenced snippet ready for an agent composer. */
export function formatAgentSelectionBlock(
  path: string,
  startLine: number,
  endLine: number,
  selectedText: string,
  promptPrefix?: string,
): string {
  const ref = formatSelectionReference(path, startLine, endLine);
  const lang = fenceLanguageForPath(path);
  const body = selectedText.replace(/\n$/, '');
  let ticks = '```';
  while (body.includes(ticks)) ticks += '`';
  const header = promptPrefix ? `${promptPrefix} \`${ref}\`` : `\`${ref}\``;
  return `${header}\n${ticks}${lang}\n${body}\n${ticks}\n`;
}

/** Picks the best conversation pane in the same workspace to receive an attached code question. */
export function pickTargetAgentSession(
  sessions: readonly RunSession[],
  workspaceId: string | undefined,
  activeSessionId: string | null | undefined,
  preferredPaneId?: string | null,
): RunSession | null {
  const isAgentPane = (s: RunSession) =>
    s.kind !== 'files' && s.kind !== 'workmap' && s.kind !== 'shell';
  const candidates = sessions.filter((s) => (!workspaceId || s.workspaceId === workspaceId) && isAgentPane(s));
  if (candidates.length === 0) return null;
  if (preferredPaneId) {
    const preferred = candidates.find((s) => s.id === preferredPaneId);
    if (preferred) return preferred;
  }
  if (activeSessionId) {
    const active = candidates.find((s) => s.id === activeSessionId);
    if (active) return active;
  }
  const idle = candidates.find((s) => s.status !== 'running');
  return idle ?? candidates[candidates.length - 1] ?? null;
}

/** Finds up to 5,000 non-overlapping occurrences of `query` in `text`. */
export function findMatches(text: string, query: string, caseSensitive: boolean): SearchMatch[] {
  if (!query || text.length === 0) return [];
  const haystack = caseSensitive ? text : text.toLowerCase();
  const needle = caseSensitive ? query : query.toLowerCase();
  if (needle.length === 0) return [];
  const out: SearchMatch[] = [];
  let pos = 0;
  let line = 1;
  let lineScanAt = 0;
  while (pos <= haystack.length - needle.length && out.length < MAXIMUM_MATCHES) {
    const found = haystack.indexOf(needle, pos);
    if (found < 0) break;
    for (let i = lineScanAt; i < found; i++) {
      if (text.charCodeAt(i) === 10) line++;
    }
    lineScanAt = found;
    out.push({ start: found, end: found + needle.length, line });
    pos = found + needle.length;
  }
  return out;
}

/** Replaces a single match and returns the updated text and caret offset right after the replacement. */
export function replaceSingleMatch(text: string, match: SearchMatch, replacement: string): { text: string; cursor: number } {
  const next = text.slice(0, match.start) + replacement + text.slice(match.end);
  return { text: next, cursor: match.start + replacement.length };
}

/** Replaces all non-overlapping matches of `query` in `text`. */
export function replaceAllMatches(
  text: string,
  query: string,
  replacement: string,
  caseSensitive: boolean,
): { text: string; count: number } {
  const matches = findMatches(text, query, caseSensitive);
  if (matches.length === 0) return { text, count: 0 };
  let out = '';
  let cursor = 0;
  for (const m of matches) {
    out += text.slice(cursor, m.start) + replacement;
    cursor = m.end;
  }
  out += text.slice(cursor);
  return { text: out, count: matches.length };
}

/** Indents (`Tab`) or outdents (`Shift+Tab`) the selected line(s) or inserts 2 spaces at a collapsed caret. */
export function indentSelection(
  text: string,
  selectionStart: number,
  selectionEnd: number,
  outdent: boolean,
  unit = '  ',
): { text: string; start: number; end: number } {
  const start = Math.max(0, Math.min(selectionStart, selectionEnd, text.length));
  const end = Math.max(0, Math.min(Math.max(selectionStart, selectionEnd), text.length));
  if (start === end && !outdent) {
    const next = text.slice(0, start) + unit + text.slice(end);
    return { text: next, start: start + unit.length, end: start + unit.length };
  }
  const lineStart = text.lastIndexOf('\n', Math.max(0, start - 1)) + 1;
  const effectiveEnd = end > start && text.charCodeAt(end - 1) === 10 ? end - 1 : end;
  const nextBreak = text.indexOf('\n', effectiveEnd);
  const blockEnd = nextBreak < 0 ? text.length : nextBreak;
  const block = text.slice(lineStart, blockEnd);
  const lines = block.split('\n');
  let firstDelta = 0;
  let totalDelta = 0;
  const transformed = lines.map((line, index) => {
    if (!outdent) {
      if (index === 0) firstDelta = unit.length;
      totalDelta += unit.length;
      return unit + line;
    }
    let removed = 0;
    if (line.startsWith('\t')) removed = 1;
    else {
      while (removed < unit.length && line.charCodeAt(removed) === 32) removed++;
    }
    if (index === 0) firstDelta = -removed;
    totalDelta -= removed;
    return line.slice(removed);
  });
  const nextText = text.slice(0, lineStart) + transformed.join('\n') + text.slice(blockEnd);
  const nextStart = Math.max(lineStart, start + firstDelta);
  const nextEnd = start === end ? nextStart : Math.max(nextStart, end + totalDelta);
  return { text: nextText, start: nextStart, end: nextEnd };
}

/** Inserts a newline preserving the current line's leading whitespace. */
export function newlineWithIndent(text: string, selectionStart: number, selectionEnd: number): { text: string; cursor: number } {
  const start = Math.max(0, Math.min(selectionStart, selectionEnd, text.length));
  const end = Math.max(0, Math.min(Math.max(selectionStart, selectionEnd), text.length));
  const lineStart = text.lastIndexOf('\n', Math.max(0, start - 1)) + 1;
  let indent = '';
  for (let i = lineStart; i < start; i++) {
    const ch = text[i];
    if (ch === ' ' || ch === '\t') indent += ch;
    else break;
  }
  const insert = '\n' + indent;
  return { text: text.slice(0, start) + insert + text.slice(end), cursor: start + insert.length };
}

const KEYWORDS = new Set([
  'as', 'async', 'await', 'break', 'case', 'catch', 'class', 'const', 'continue', 'def', 'default', 'defer',
  'delete', 'do', 'else', 'enum', 'export', 'extends', 'false', 'final', 'finally', 'fn', 'for', 'from',
  'func', 'function', 'guard', 'if', 'impl', 'implements', 'import', 'in', 'interface', 'let', 'loop',
  'match', 'mod', 'mut', 'namespace', 'new', 'nil', 'None', 'null', 'override', 'package', 'private',
  'protected', 'pub', 'public', 'readonly', 'return', 'self', 'Self', 'Some', 'static', 'struct', 'super',
  'switch', 'this', 'throw', 'throws', 'trait', 'true', 'try', 'type', 'typeof', 'undefined', 'use', 'val',
  'var', 'void', 'where', 'while', 'with', 'yield',
]);

function isWordStart(code: number): boolean {
  return (code >= 65 && code <= 90) || (code >= 97 && code <= 122) || code === 95 || code === 36;
}

function isWordPart(code: number): boolean {
  return isWordStart(code) || (code >= 48 && code <= 57);
}

/** Fast single-pass syntax + search-match tokenizer for the lightweight editor's backdrop (`< 256 KB`). */
export function renderEditorBackdropSegments(
  text: string,
  path: string,
  matches: readonly SearchMatch[],
  currentMatchIndex: number,
): BackdropSegment[] {
  const lang = fenceLanguageForPath(path);
  const plainOnly = lang === '' || lang === 'markdown';
  const hashComment = lang === 'python' || lang === 'bash' || lang === 'zsh' || lang === 'yaml' || lang === 'toml' || lang === 'makefile' || lang === 'dockerfile';
  const slashComment = !plainOnly && !hashComment && lang !== 'html' && lang !== 'xml';

  // Build coarse syntax spans [start, end, kind].
  const spans: Array<{ start: number; end: number; kind: HighlightKind }> = [];
  if (plainOnly || text.length > 260_000) {
    if (text.length > 0) spans.push({ start: 0, end: text.length, kind: 'plain' });
  } else {
    let i = 0;
    let plainStart = 0;
    const flushPlain = (at: number) => {
      if (at > plainStart) spans.push({ start: plainStart, end: at, kind: 'plain' });
    };
    while (i < text.length) {
      const ch = text.charCodeAt(i);
      const next = i + 1 < text.length ? text.charCodeAt(i + 1) : 0;
      if (slashComment && ch === 47 && next === 47) {
        flushPlain(i);
        const end = text.indexOf('\n', i + 2);
        const stop = end < 0 ? text.length : end;
        spans.push({ start: i, end: stop, kind: 'comment' });
        i = stop;
        plainStart = i;
        continue;
      }
      if (slashComment && ch === 47 && next === 42) {
        flushPlain(i);
        const end = text.indexOf('*/', i + 2);
        const stop = end < 0 ? text.length : end + 2;
        spans.push({ start: i, end: stop, kind: 'comment' });
        i = stop;
        plainStart = i;
        continue;
      }
      if (hashComment && ch === 35) {
        flushPlain(i);
        const end = text.indexOf('\n', i + 1);
        const stop = end < 0 ? text.length : end;
        spans.push({ start: i, end: stop, kind: 'comment' });
        i = stop;
        plainStart = i;
        continue;
      }
      if (ch === 34 || ch === 39 || ch === 96) {
        flushPlain(i);
        const quote = ch;
        let j = i + 1;
        while (j < text.length) {
          const c = text.charCodeAt(j);
          if (c === 92) { j += 2; continue; }
          if (c === quote) { j++; break; }
          if (quote !== 96 && c === 10) break;
          j++;
        }
        spans.push({ start: i, end: j, kind: 'string' });
        i = j;
        plainStart = i;
        continue;
      }
      if (ch >= 48 && ch <= 57 && (i === 0 || !isWordPart(text.charCodeAt(i - 1)))) {
        flushPlain(i);
        let j = i + 1;
        while (j < text.length && (isWordPart(text.charCodeAt(j)) || text.charCodeAt(j) === 46)) j++;
        spans.push({ start: i, end: j, kind: 'number' });
        i = j;
        plainStart = i;
        continue;
      }
      if (isWordStart(ch)) {
        let j = i + 1;
        while (j < text.length && isWordPart(text.charCodeAt(j))) j++;
        const word = text.slice(i, j);
        if (KEYWORDS.has(word)) {
          flushPlain(i);
          spans.push({ start: i, end: j, kind: 'keyword' });
          plainStart = j;
        }
        i = j;
        continue;
      }
      i++;
    }
    flushPlain(text.length);
  }

  if (matches.length === 0) {
    return spans.map((s) => ({ text: text.slice(s.start, s.end), token: s.kind, match: 'none' }));
  }

  // Intersect syntax spans with search matches so search highlights layer cleanly over syntax tokens.
  const out: BackdropSegment[] = [];
  let mIdx = 0;
  for (const span of spans) {
    let cursor = span.start;
    while (mIdx < matches.length && (matches[mIdx]?.end ?? 0) <= cursor) mIdx++;
    let scanIdx = mIdx;
    while (cursor < span.end && scanIdx < matches.length) {
      const m = matches[scanIdx]!;
      if (m.start >= span.end) break;
      if (m.start > cursor) {
        out.push({ text: text.slice(cursor, Math.min(span.end, m.start)), token: span.kind, match: 'none' });
        cursor = m.start;
      }
      const segEnd = Math.min(span.end, m.end);
      if (segEnd > cursor) {
        out.push({
          text: text.slice(cursor, segEnd),
          token: span.kind,
          match: scanIdx === currentMatchIndex ? 'current' : 'match',
        });
        cursor = segEnd;
      }
      if (m.end <= span.end) scanIdx++;
      else break;
    }
    if (cursor < span.end) {
      out.push({ text: text.slice(cursor, span.end), token: span.kind, match: 'none' });
    }
  }
  return out;
}

function splitDiffLines(content: string): { lines: string[]; trailingNewline: boolean } {
  if (content.length === 0) return { lines: [], trailingNewline: true };
  const normalized = content.replace(/\r\n/g, '\n');
  const trailingNewline = normalized.endsWith('\n');
  const raw = normalized.split('\n');
  if (trailingNewline) raw.pop();
  return { lines: raw, trailingNewline };
}

/**
 * Computes an in-memory unified `FileChanges` between `baseText` and `draftText` (up to 3 context lines per hunk) so the files
 * pane can switch instantaneously between `편집` and `Diff` even before saving to disk.
 */
export function computeBufferChanges(baseText: string, draftText: string, headSha?: string): FileChanges {
  const normBase = baseText.replace(/\r\n/g, '\n');
  const normDraft = draftText.replace(/\r\n/g, '\n');
  if (normBase === normDraft) {
    return { base: 'head', hunks: [], added: 0, removed: 0, truncated: false, unchanged: true, headSha };
  }
  const oldSide = splitDiffLines(normBase);
  const newSide = splitDiffLines(normDraft);
  const a = oldSide.lines;
  const b = newSide.lines;

  // Trim shared prefix and suffix first so typical localized edits are O(changed region).
  let prefix = 0;
  while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix]) prefix++;
  // If trailing newline status differs on the final line, don't merge the very last line into the unchanged suffix.
  const newlineDiff = oldSide.trailingNewline !== newSide.trailingNewline;
  let suffix = 0;
  const maxSuffix = Math.min(a.length - prefix, b.length - prefix) - (newlineDiff ? 1 : 0);
  while (suffix < maxSuffix && a[a.length - 1 - suffix] === b[b.length - 1 - suffix]) suffix++;

  const midA = a.slice(prefix, a.length - Math.max(0, suffix));
  const midB = b.slice(prefix, b.length - Math.max(0, suffix));

  type EditOp = { op: ' ' | '+' | '-'; text: string; oldIdx: number; newIdx: number; noNewline?: boolean };
  const ops: EditOp[] = [];
  for (let i = 0; i < prefix; i++) {
    ops.push({ op: ' ', text: a[i]!, oldIdx: i, newIdx: i });
  }

  if (midA.length * midB.length <= 400_000) {
    const rows = midA.length;
    const cols = midB.length;
    const dp = new Uint32Array((rows + 1) * (cols + 1));
    const stride = cols + 1;
    for (let i = rows - 1; i >= 0; i--) {
      for (let j = cols - 1; j >= 0; j--) {
        dp[i * stride + j] = midA[i] === midB[j]
          ? (dp[(i + 1) * stride + (j + 1)] ?? 0) + 1
          : Math.max(dp[(i + 1) * stride + j] ?? 0, dp[i * stride + (j + 1)] ?? 0);
      }
    }
    let i = 0;
    let j = 0;
    while (i < rows && j < cols) {
      if (midA[i] === midB[j]) {
        ops.push({ op: ' ', text: midA[i]!, oldIdx: prefix + i, newIdx: prefix + j });
        i++;
        j++;
      } else if ((dp[(i + 1) * stride + j] ?? 0) >= (dp[i * stride + (j + 1)] ?? 0)) {
        ops.push({ op: '-', text: midA[i]!, oldIdx: prefix + i, newIdx: prefix + j });
        i++;
      } else {
        ops.push({ op: '+', text: midB[j]!, oldIdx: prefix + i, newIdx: prefix + j });
        j++;
      }
    }
    while (i < rows) {
      ops.push({ op: '-', text: midA[i]!, oldIdx: prefix + i, newIdx: prefix + cols });
      i++;
    }
    while (j < cols) {
      ops.push({ op: '+', text: midB[j]!, oldIdx: prefix + rows, newIdx: prefix + j });
      j++;
    }
  } else {
    for (let i = 0; i < midA.length; i++) {
      ops.push({ op: '-', text: midA[i]!, oldIdx: prefix + i, newIdx: prefix });
    }
    for (let j = 0; j < midB.length; j++) {
      ops.push({ op: '+', text: midB[j]!, oldIdx: prefix + midA.length, newIdx: prefix + j });
    }
  }

  const safeSuffix = Math.max(0, suffix);
  for (let s = 0; s < safeSuffix; s++) {
    const oi = a.length - safeSuffix + s;
    const ni = b.length - safeSuffix + s;
    ops.push({ op: ' ', text: a[oi]!, oldIdx: oi, newIdx: ni });
  }

  // Mark missing final newline on whichever side's final line was emitted.
  if (!oldSide.trailingNewline && a.length > 0) {
    for (let k = ops.length - 1; k >= 0; k--) {
      if (ops[k]!.op !== '+') { ops[k]!.noNewline = true; break; }
    }
  }
  if (!newSide.trailingNewline && b.length > 0) {
    for (let k = ops.length - 1; k >= 0; k--) {
      if (ops[k]!.op !== '-') { ops[k]!.noNewline = true; break; }
    }
  }

  let added = 0;
  let removed = 0;
  const changedIndices: number[] = [];
  ops.forEach((entry, idx) => {
    if (entry.op === '+') { added++; changedIndices.push(idx); }
    else if (entry.op === '-') { removed++; changedIndices.push(idx); }
  });
  if (changedIndices.length === 0 && newlineDiff && ops.length > 0) {
    changedIndices.push(ops.length - 1);
  }

  const CONTEXT = 3;
  const groups: Array<[number, number]> = [];
  for (const idx of changedIndices) {
    const from = Math.max(0, idx - CONTEXT);
    const to = Math.min(ops.length - 1, idx + CONTEXT);
    const last = groups[groups.length - 1];
    if (last && from <= last[1] + 1) last[1] = Math.max(last[1], to);
    else groups.push([from, to]);
  }

  const hunks: FileDiffHunk[] = [];
  let emitted = 0;
  let truncated = false;
  for (const [from, to] of groups) {
    const slice = ops.slice(from, to + 1);
    if (emitted + slice.length > MAXIMUM_DIFF_LINES) {
      truncated = true;
      break;
    }
    emitted += slice.length;
    const lines: FileDiffLine[] = slice.map((item) => ({
      op: item.op,
      text: item.text,
      ...(item.noNewline ? { noNewline: true } : {}),
    }));
    const oldLines = slice.filter((item) => item.op !== '+').length;
    const newLines = slice.filter((item) => item.op !== '-').length;
    const first = slice[0]!;
    const oldStart = oldLines === 0 ? first.oldIdx : first.oldIdx + 1;
    const newStart = newLines === 0 ? first.newIdx : first.newIdx + 1;
    hunks.push({ oldStart, oldLines, newStart, newLines, lines });
  }

  return {
    base: 'head',
    hunks,
    added,
    removed,
    truncated,
    unchanged: hunks.length === 0,
    headSha,
  };
}

// 요청 변경 검토 (M/RunChangesSheet.swift, M/MightyCore/ReviewNotes.swift): what the review panel reads and works out without the
// browser. The host answers `runChangesRuns`, `runChanges` and `runChangesRevert` (files.rs); the diff of a file is `referenceChanges`.
import type { MightyGraphRun } from '../store/models';
import type { DiffRow } from '../panes/files/diff';

/** Why a file cannot be put back now (mighty-files revert.rs `Refusal`). */
export type RunChangesRefusal = 'laterRun' | 'missing' | 'unreadable' | 'unchanged';

/** One file the request changed, as reverting it would go (`runChanges`'s items). */
export interface RunChangesItem {
  path: string;
  /** `trash`: the request made the file, a revert moves it to the Trash; `restore`: its text before the request is written back. */
  action: 'restore' | 'trash';
  /** Lines a revert puts back (the request removed them) and takes out (the request added them). */
  restored: number;
  removed: number;
  refusal?: RunChangesRefusal;
}

export interface RunChangesPlan { root: string; items: RunChangesItem[] }

/** How one file's revert went (`runChangesRevert`'s outcomes). */
export type RunChangesOutcome = { path: string } & ({ kind: 'reverted' } | { kind: 'refused'; refusal: RunChangesRefusal } | { kind: 'failed'; error: string });

export interface RunChangesReverted { outcomes: RunChangesOutcome[]; items: RunChangesItem[] }

/** A run settled: its result block may offer the review (Swift: completed, failed or stopped). */
const Settled = new Set(['completed', 'failed', 'error', 'stopped', 'cancelled', 'interrupted']);

/** The ids of the runs whose result blocks may offer the review, as one key that changes only when that list does. */
export function settledRunsKey(runs: readonly Pick<MightyGraphRun, 'id' | 'status'>[]): string {
  return runs.filter((run) => Settled.has(run.status)).map((run) => run.id).join('\n');
}

export function revertible(items: readonly RunChangesItem[]): RunChangesItem[] {
  return items.filter((item) => item.refusal === undefined);
}

/** A remark the person left on one line of a request's diff, sent back to the agent as a follow-up request. */
export interface ReviewNote {
  id: number;
  path: string;
  /** The line on its side: the file now (`new`) or before the request (`old`, a removed line). */
  line: number;
  side: 'old' | 'new';
  /** The line's text as the diff showed it. */
  quote: string;
  text: string;
}

/** Quoted lines longer than this are cut with an ellipsis. */
export const MaximumQuote = 120;

/** The note a diff row can carry: an added or kept line by its number now, a removed one by its number before; none on a hunk's head. */
export function noteTarget(row: DiffRow): { line: number; side: 'old' | 'new' } | null {
  if (row.kind === 'removed') return row.old !== null ? { line: row.old, side: 'old' } : null;
  if (row.kind === 'added' || row.kind === 'context') return row.new !== null ? { line: row.new, side: 'new' } : null;
  return null;
}

/** Where a note sits, as the agent can open it: `path:line`, a removed line `path −line`. */
export function notePlace(path: string, target: { line: number; side: 'old' | 'new' }): string {
  return target.side === 'new' ? `${path}:${target.line}` : `${path} −${target.line}`;
}

/** The indexes of the shown diff's rows that carry a note of `path`, matched by side and line number. */
export function notedRows(rows: readonly DiffRow[], notes: readonly ReviewNote[], path: string): Set<number> {
  const here = notes.filter((note) => note.path === path);
  const out = new Set<number>();
  rows.forEach((row, index) => {
    const target = noteTarget(row);
    if (target && here.some((note) => note.side === target.side && note.line === target.line)) out.add(index);
  });
  return out;
}

/**
 * Swift `ReviewNotes.prompt`: the follow-up request the notes become: a header line, then one bullet per note in file and line order,
 * each naming the place as the agent can open it (`path:line`; a removed line by its number before the request, `removedLabel` with
 * `{line}`), the quoted line after it and the note under it. Empty notes are left out; none left makes no request.
 */
export function reviewPrompt(notes: readonly ReviewNote[], header: string, removedLabel: string): string {
  const kept = notes.map((note, index) => ({ note, index })).filter(({ note }) => note.text.trim() !== '');
  if (kept.length === 0) return '';
  kept.sort((a, b) => (a.note.path < b.note.path ? -1 : a.note.path > b.note.path ? 1 : a.note.line - b.note.line || a.index - b.index));
  const lines = [header];
  for (const { note } of kept) {
    const place = note.side === 'new' ? '`' + note.path + ':' + note.line + '`' : '`' + note.path + '` (' + removedLabel.split('{line}').join(String(note.line)) + ')';
    const quote = note.quote.replace(/^[ \t]+|[ \t]+$/g, '');
    const chars = [...quote];
    const shown = chars.length > MaximumQuote ? chars.slice(0, MaximumQuote).join('') + '…' : quote;
    const body = note.text.trim().split('\n').join('\n  ');
    lines.push('- ' + place + (shown === '' ? '' : ' — `' + shown.split('`').join("'") + '`') + '\n  ' + body);
  }
  return lines.join('\n');
}

/** Swift `ReviewNotes.appending`: `block` after what the composer already holds, a blank line between. */
export function appendingBlock(block: string, draft: string): string {
  if (block === '') return draft;
  const trimmed = draft.trim();
  return trimmed === '' ? block : trimmed + '\n\n' + block;
}

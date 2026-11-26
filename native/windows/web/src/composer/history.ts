// MC-106: the editor's ↑ and ↓ walk the pane's earlier requests, as a shell's history does. ↑ on the editor's first line brings the
// request before the one shown (the words being typed are kept aside first); ↓ on its last line brings the one after, and past the
// newest the kept words come back. Typing in a recalled request makes it the words being typed. Pure; the editor holds the state.
import type { MightyGraphRun } from '../store/models';

/** Where the walk stands: `index` into the requests (null: the words being typed are shown) and those words, kept while walking. */
export interface HistoryWalk {
  index: number | null;
  draft: string;
}

export const atDraft: HistoryWalk = { index: null, draft: '' };

/** The pane's requests, oldest first: what the person sent (not Claude's replies to background notices), each once in a row. */
export function requestHistory(runs: readonly MightyGraphRun[] | undefined): string[] {
  const out: string[] = [];
  for (const run of runs ?? []) {
    if (run.noticeRun) continue;
    const words = run.input.trim();
    if (words.length > 0 && out[out.length - 1] !== words) out.push(words);
  }
  return out;
}

/** Whether the caret stands on the text's first line (nothing but the same line before it). */
export const onFirstLine = (text: string, caret: number) => !text.slice(0, caret).includes('\n');
/** Whether the caret stands on the text's last line (nothing but the same line after it). */
export const onLastLine = (text: string, caret: number) => !text.slice(caret).includes('\n');

/** ↑: the request before the one shown, keeping the typed words when the walk starts; null when there is none further back. */
export function walkUp(walk: HistoryWalk, history: readonly string[], text: string): { walk: HistoryWalk; text: string } | null {
  if (history.length === 0 || walk.index === 0) return null;
  const index = walk.index === null ? history.length - 1 : walk.index - 1;
  const draft = walk.index === null ? text : walk.draft;
  return { walk: { index, draft }, text: history[index] ?? '' };
}

/** ↓: the request after the one shown, and past the newest the kept words; null when the typed words are already shown. */
export function walkDown(walk: HistoryWalk, history: readonly string[]): { walk: HistoryWalk; text: string } | null {
  if (walk.index === null) return null;
  const index = walk.index + 1;
  if (index >= history.length) return { walk: atDraft, text: walk.draft };
  return { walk: { index, draft: walk.draft }, text: history[index] ?? '' };
}

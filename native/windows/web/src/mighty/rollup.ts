// A request's 결과물 (Core checkpoints.rs): what each part of the work changed, gathered for its last result. Each step done, each
// background turn and the run's end left a snapshot of the folder with the files changed since the one before, whichever agent ran
// it; this groups them for the result's list, by step or by file, kept short when there are many. Pure.
import type { CheckpointFile, MightyGraphRun, RunCheckpoint } from '../store/models';

/** The request a run belongs to: a notice run `<run>:notice:<n>` stands for the run it came from (notice.ts). */
export function requestOf(id: string): string {
  const at = id.indexOf(':notice:');
  return at < 0 ? id : id.slice(0, at);
}

/** What one part of the work changed. */
export interface RollUpSection {
  /** Unique within the request: the checkpoint's run and tree. */
  key: string;
  kind: 'step' | 'turn' | 'end';
  /** The step's text, the background task's name; absent for an end. */
  label?: string;
  at?: string;
  runId: string;
  base: string;
  tree: string;
  files: CheckpointFile[];
  /** Files changed past the listed ones. */
  more: number;
}

/** A file over the whole request (the 파일별 view): the sections that changed it, by their index, and its lines in all of them. */
export interface RolledFile {
  path: string;
  status: string;
  oldPath?: string;
  added?: number;
  removed?: number;
  sections: number[];
}

/** Whether a later run of the same request follows `run` (its result is then a 중간 결과, view_model.rs `followed`). */
export function followed(run: MightyGraphRun, runs: readonly MightyGraphRun[]): boolean {
  const request = requestOf(run.id);
  const at = runs.findIndex((r) => r.id === run.id);
  return at >= 0 && runs.slice(at + 1).some((later) => later.id !== request && requestOf(later.id) === request);
}

/**
 * The sections of `run`'s request, oldest first: every checkpoint of the request's own run and of its notice runs that saw files
 * change. None for a result a later turn of the request follows: the last result gathers them.
 */
export function rollUp(runs: readonly MightyGraphRun[], run: MightyGraphRun): RollUpSection[] {
  if (followed(run, runs)) return [];
  const request = requestOf(run.id);
  const sections: RollUpSection[] = [];
  for (const each of runs) {
    if (requestOf(each.id) !== request) continue;
    for (const checkpoint of each.checkpoints ?? []) {
      if (checkpoint.kind === 'start' || !checkpoint.base || checkpoint.files.length === 0) continue;
      sections.push(section(each.id, checkpoint));
    }
  }
  return sections;
}

function section(runId: string, checkpoint: RunCheckpoint): RollUpSection {
  const kind = checkpoint.kind === 'step' || checkpoint.kind === 'turn' ? checkpoint.kind : 'end';
  return {
    key: runId + ':' + checkpoint.kind + ':' + checkpoint.tree,
    kind,
    label: checkpoint.label,
    at: checkpoint.at,
    runId,
    base: checkpoint.base!,
    tree: checkpoint.tree,
    files: checkpoint.files,
    more: checkpoint.more ?? 0,
  };
}

/** The files over all sections, each once (its latest status and path), in the order they first changed. */
export function byFile(sections: readonly RollUpSection[]): RolledFile[] {
  const files = new Map<string, RolledFile>();
  sections.forEach((section, index) => {
    for (const file of section.files) {
      // A file renamed in a later step is the same file: it is found by its old path.
      const known = files.get(file.path) ?? (file.oldPath ? files.get(file.oldPath) : undefined);
      if (!known) {
        files.set(file.path, { path: file.path, status: file.status, oldPath: file.oldPath, added: file.added, removed: file.removed, sections: [index] });
        continue;
      }
      if (known.path !== file.path) {
        files.delete(known.path);
        known.path = file.path;
        files.set(file.path, known);
      }
      // Added then changed is still new; changed then deleted is deleted.
      known.status = known.status === 'A' && file.status !== 'D' ? 'A' : file.status;
      known.added = known.added === undefined || file.added === undefined ? undefined : known.added + file.added;
      known.removed = known.removed === undefined || file.removed === undefined ? undefined : known.removed + file.removed;
      if (!known.sections.includes(index)) known.sections.push(index);
    }
  });
  return [...files.values()];
}

/** The rows of a list under folder headings: a folder holding at least `least` of the files gets a heading; the rest stay as they are. */
export function foldered<T extends { path: string }>(files: readonly T[], least = 3): ({ folder: string } | { file: T })[] {
  const folderOf = (path: string) => { const at = path.lastIndexOf('/'); return at < 0 ? '' : path.slice(0, at); };
  const counts = new Map<string, number>();
  for (const file of files) counts.set(folderOf(file.path), (counts.get(folderOf(file.path)) ?? 0) + 1);
  const rows: ({ folder: string } | { file: T })[] = [];
  const grouped = new Map<string, T[]>();
  const loose: T[] = [];
  for (const file of files) {
    const folder = folderOf(file.path);
    if (folder !== '' && (counts.get(folder) ?? 0) >= least) grouped.set(folder, [...(grouped.get(folder) ?? []), file]);
    else loose.push(file);
  }
  for (const [folder, inside] of grouped) {
    rows.push({ folder });
    for (const file of inside) rows.push({ file });
  }
  for (const file of loose) rows.push({ file });
  return rows;
}

/** Whether a file's path holds every word of `query` (case aside). */
export function matches(path: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const lower = path.toLowerCase();
  return words.every((word) => lower.includes(word));
}

/**
 * Which sections start open: all of them when there are few (four or fewer), else the last two, where the request ended. The rest
 * are a click away.
 */
export function openAtFirst(count: number): Set<number> {
  if (count <= 4) return new Set(Array.from({ length: count }, (_, index) => index));
  return new Set([count - 2, count - 1]);
}

/** How many rows a section shows before its 더 보기. */
export const SECTION_ROWS = 5;
/** Past this many files in all, the list gets a search box. */
export const SEARCH_FROM = 20;

// Pure data models and formatting / selection helpers for the Git Workbench panel (MC-5).
import type { IconName } from '../icons/Icon';

export type GitHosting = 'github' | 'gitlab' | 'none';
export type GitCheckRollup = 'passed' | 'failed' | 'pending' | 'none';
export type GitPrState = 'open' | 'merged' | 'closed';

export interface GitWorkbenchFile {
  path: string;
  originalPath?: string;
  mark: string;
  indexStatus: string;
  worktreeStatus: string;
  staged: boolean;
  unstaged: boolean;
  untracked: boolean;
  conflicted: boolean;
}

export interface GitWorkbenchStatus {
  workspaceId: string;
  head: string;
  detached: boolean;
  upstream?: string | null;
  ahead: number;
  behind: number;
  branches: string[];
  hosting: GitHosting;
  remoteUrl?: string | null;
  /** The branch pull requests are made into (origin's HEAD, else main or master). */
  defaultBranch?: string | null;
  /** HEAD is the default branch or the base of this pane's worktree: no pull request is made from it. */
  onDefaultBranch?: boolean;
  files: GitWorkbenchFile[];
}

export interface GitDiffLine {
  kind: 'context' | 'add' | 'del';
  oldLine: number | null;
  newLine: number | null;
  text: string;
}

export interface GitDiffHunk {
  index: number;
  header: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  rawPatch: string;
  lines: GitDiffLine[];
}

export interface GitWorkbenchDiff {
  path: string;
  staged: boolean;
  hunks: GitDiffHunk[];
  added: number;
  removed: number;
  binary: boolean;
  truncated: boolean;
}

export interface SecretFinding {
  path: string;
  line: number;
  kind: string;
}

export interface GitCommitResult {
  committed: boolean;
  blockedBySecrets?: boolean;
  sha?: string;
  findings: SecretFinding[];
}

export interface GitDraftMessageResult {
  message: string;
  source: 'ai' | 'heuristic';
}

export interface GitPrInfo {
  number: number;
  url: string;
  state: GitPrState | string;
  title: string;
  isDraft: boolean;
  checks: GitCheckRollup;
}

export interface GitPrStatusResult {
  hosting: GitHosting;
  pr: GitPrInfo | null;
  cliMissing?: 'gh' | 'glab';
}

export interface GitCreatePrResult {
  hosting: GitHosting;
  url?: string | null;
  pr: GitPrInfo | null;
}

export interface GitFailedCheckLogResult {
  log: string;
  lines: number;
}

export interface CheckBadgeDescriptor {
  icon: IconName;
  tone: 'done' | 'err' | 'wait';
  labelKey: string;
}

/**
 * Preserves existing file selections across status refreshes, automatically selecting
 * newly discovered non-conflicted files while keeping conflicted files unselected by default.
 */
export function reconcileSelectedPaths(
  previousSelected: ReadonlySet<string>,
  previousKnownPaths: ReadonlySet<string>,
  nextFiles: readonly GitWorkbenchFile[],
): Set<string> {
  const next = new Set<string>();
  for (const file of nextFiles) {
    if (file.conflicted) continue;
    if (previousSelected.has(file.path) || !previousKnownPaths.has(file.path)) {
      next.add(file.path);
    }
  }
  return next;
}

/** Toggles between selecting all non-conflicted files and clearing the selection. */
export function toggleAllSelectablePaths(
  files: readonly GitWorkbenchFile[],
  currentSelected: ReadonlySet<string>,
): Set<string> {
  const selectable = files.filter((f) => !f.conflicted);
  const allSelected = selectable.length > 0 && selectable.every((f) => currentSelected.has(f.path));
  if (allSelected) return new Set<string>();
  return new Set(selectable.map((f) => f.path));
}

/** Counts how many changed files currently have staged index changes. */
export function countStagedFiles(files: readonly GitWorkbenchFile[]): number {
  let count = 0;
  for (const file of files) {
    if (file.staged) count += 1;
  }
  return count;
}

/** Picks whether a file should default to showing the staged diff tab (`true` when staged-only). */
export function defaultDiffStagedTab(file: GitWorkbenchFile | null | undefined, preferredStaged: boolean): boolean {
  if (!file) return false;
  if (file.staged && !file.unstaged) return true;
  if (!file.staged && file.unstaged) return false;
  return preferredStaged;
}

/** Maps a file status mark (`A`, `?`, `D`, `!`, `R`, `M`) to a semantic token tone. */
export function fileMarkTone(mark: string): 'done' | 'err' | 'wait' {
  if (mark === 'D' || mark === '!') return 'err';
  if (mark === 'A' || mark === '?') return 'done';
  return 'wait';
}

/** Maps CI check rollup states to icon, semantic tone, and locale key (`null` when `none`). */
export function checkBadgeMeta(checks: GitCheckRollup | string | undefined): CheckBadgeDescriptor | null {
  switch (checks) {
    case 'passed':
      return { icon: 'circle-check', tone: 'done', labelKey: 'git.pr.checks.passed' };
    case 'failed':
      return { icon: 'circle-x', tone: 'err', labelKey: 'git.pr.checks.failed' };
    case 'pending':
      return { icon: 'clock', tone: 'wait', labelKey: 'git.pr.checks.pending' };
    default:
      return null;
  }
}

/** Maps PR/MR state (`open`, `merged`, `closed`) to its existing locale key. */
export function prStateLocaleKey(state: string): string | null {
  switch (state.toLowerCase()) {
    case 'open':
    case 'opened':
      return 'git.pr.state.open';
    case 'merged':
      return 'git.pr.state.merged';
    case 'closed':
      return 'git.pr.state.closed';
    default:
      return null;
  }
}

/** Formats the agent commit request (`git.commit.agentRequest`) with a Markdown list of target files. */
export function formatAgentCommitPrompt(requestHeader: string, paths: readonly string[]): string {
  const cleanPaths = paths.map((p) => p.trim()).filter((p) => p.length > 0);
  if (cleanPaths.length === 0) return requestHeader.trim();
  const list = cleanPaths.map((p) => `- \`${p}\``).join('\n');
  return `${requestHeader.trim()}\n${list}`;
}

/**
 * Formats the CI failure fix prompt (`git.pr.fixRequest`) and wraps the 80-line log tail
 * in a collision-safe fenced block (expanding backticks if the log itself contains triple backticks).
 */
export function formatCiFixPrompt(fixRequestHeader: string, rawLog?: string | null): string {
  const header = fixRequestHeader.trim();
  const trimmedLog = (rawLog ?? '').replace(/\r\n/g, '\n').trim();
  if (trimmedLog.length === 0) return header;
  let fence = '```';
  while (trimmedLog.includes(fence)) {
    fence += '`';
  }
  return `${header}\n\n${fence}text\n${trimmedLog}\n${fence}`;
}

/** Formats secret scan findings (`path:line · kind`) without exposing secret values. */
export function formatSecretFindings(findings: readonly SecretFinding[], maxVisible = 8): string[] {
  const lines = findings.slice(0, Math.max(1, maxVisible)).map((f) =>
    f.line > 0 ? `${f.path}:${f.line} · ${f.kind}` : `${f.path} · ${f.kind}`,
  );
  if (findings.length > maxVisible) {
    lines.push('…');
  }
  return lines;
}

/** Client-side check for valid local git branch names before calling `gitWorkbenchSwitchBranch`. */
export function isValidBranchName(raw: string): boolean {
  const name = raw.trim();
  if (name.length === 0 || name.length > 240) return false;
  if (name === '@' || name.startsWith('-') || name.startsWith('/') || name.endsWith('/') || name.endsWith('.')) {
    return false;
  }
  if (name.endsWith('.lock') || name.includes('..') || name.includes('@{') || name.includes('//')) {
    return false;
  }
  if (/[\s\u0000-\u001f\u007f\\~^:?*[]/.test(name)) return false;
  return name.split('/').every((segment) => segment.length > 0 && !segment.startsWith('.') && !segment.endsWith('.lock'));
}

/** Validates external HTTP/HTTPS URLs before opening in the system browser. */
export function safeExternalHttpUrl(raw?: string | null): string | null {
  if (!raw) return null;
  try {
    const parsed = new URL(raw);
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed.toString() : null;
  } catch {
    return null;
  }
}

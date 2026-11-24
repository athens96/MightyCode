// Pre-merge checks (MC-27; src-tauri merge_checks.rs): the model `gitWorkbenchChecks` answers, the words of its warnings and of a part that
// could not be read, and the follow-up the chosen review threads become in the composer.
import type { IconName } from '../icons/Icon';

/** Why a part could not be read (shown as "can't check" with the reason). */
export interface ChecksUnknown {
  ok: false;
  reason: 'noRemote' | 'cliMissing' | 'notLoggedIn' | 'noPr' | 'failed' | string;
  cli?: string;
  detail?: string;
}

export type ChecksPart<T> = { ok: true; value: T } | ChecksUnknown;

export interface ChecksGit {
  head: string;
  base?: string | null;
  uncommitted: number;
  conflicted: number;
  ahead: number;
  behind: number;
}

export interface ChecksPr {
  hosting: 'github' | 'gitlab' | string;
  number: number;
  url: string;
  state: 'open' | 'merged' | 'closed' | string;
  title: string;
  isDraft: boolean;
  mergeable: 'mergeable' | 'conflicting' | 'unknown' | string;
  reviewDecision: 'approved' | 'changesRequested' | 'reviewRequired' | '' | string;
}

export type CheckState = 'passed' | 'failed' | 'pending' | 'skipped';

export interface ChecksCiItem {
  name: string;
  state: CheckState | string;
  url: string;
}

export interface ChecksThread {
  id: string;
  path: string;
  line: number | null;
  outdated: boolean;
  author: string;
  /** The first comment, redacted by the host. */
  body: string;
  url: string;
  replies: number;
}

export interface ChecksChecklist {
  total: number;
  remaining: string[];
}

export type ChecksWarningCode =
  | 'uncommitted' | 'draft' | 'conflicts' | 'changesRequested' | 'checksFailed' | 'checksPending' | 'threads' | 'checklist';

export interface ChecksWarning {
  code: ChecksWarningCode | string;
  count: number;
}

export interface ChecksModel {
  git: ChecksPart<ChecksGit>;
  pr: ChecksPart<ChecksPr>;
  checks: ChecksPart<ChecksCiItem[]>;
  threads: ChecksPart<ChecksThread[]>;
  checklist: ChecksPart<ChecksChecklist>;
  warnings: ChecksWarning[];
  workspaceId?: string;
  paneId?: string | null;
}

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

/** How often the tab reads again while it is open. */
export const ChecksRefreshMs = 30_000;

/** The longest follow-up the threads become. */
export const FollowUpLimit = 2_000;

/** Each warning's words, named in full so the locale check sees them used. */
const warningKeys: Record<ChecksWarningCode, string> = {
  uncommitted: 'git.checks.warn.uncommitted',
  draft: 'git.checks.warn.draft',
  conflicts: 'git.checks.warn.conflicts',
  changesRequested: 'git.checks.warn.changesRequested',
  checksFailed: 'git.checks.warn.checksFailed',
  checksPending: 'git.checks.warn.checksPending',
  threads: 'git.checks.warn.threads',
  checklist: 'git.checks.warn.checklist',
};

/** The warnings as lines (the review card's help, the tab's summary). */
export function warningLines(warnings: readonly ChecksWarning[], t: Translate): string[] {
  return warnings.map((warning) => t(warningKeys[warning.code as ChecksWarningCode] ?? 'git.checks.warn.checklist', { count: warning.count }));
}

/** Why a part could not be read, in words (the line under "can't check"). */
export function unknownReason(unknown: ChecksUnknown, t: Translate): string {
  switch (unknown.reason) {
    case 'noRemote': return t('git.pr.noRemote');
    case 'cliMissing': return t(unknown.cli === 'glab' ? 'git.pr.error.glabMissing' : 'git.pr.error.ghMissing');
    case 'notLoggedIn': return t('git.checks.reason.notLoggedIn', { cli: unknown.cli ?? 'gh' });
    case 'noPr': return t('git.pr.none');
    // MC-47: an SSH workspace's pull request is not looked up from this computer.
    case 'ssh': return t('git.checks.reason.ssh');
    default: return t('git.checks.reason.failed');
  }
}

export interface CiCounts { passed: number; failed: number; pending: number; skipped: number }

export function ciCounts(items: readonly ChecksCiItem[]): CiCounts {
  const counts: CiCounts = { passed: 0, failed: 0, pending: 0, skipped: 0 };
  for (const item of items) {
    if (item.state === 'passed' || item.state === 'failed' || item.state === 'pending') counts[item.state] += 1;
    else counts.skipped += 1;
  }
  return counts;
}

/** A check's glyph and tone. */
export function ciGlyph(state: string): { icon: IconName; tone: 'done' | 'err' | 'wait' } {
  if (state === 'passed') return { icon: 'circle-check', tone: 'done' };
  if (state === 'failed') return { icon: 'circle-x', tone: 'err' };
  if (state === 'pending') return { icon: 'clock', tone: 'wait' };
  return { icon: 'minus', tone: 'wait' };
}

/** Where a thread sits, as the agent can open it: `path:line`, `path`, or nothing for a comment on the whole request. */
export function threadPlace(thread: Pick<ChecksThread, 'path' | 'line'>): string {
  if (!thread.path) return '';
  return thread.line !== null && thread.line > 0 ? `${thread.path}:${thread.line}` : thread.path;
}

/**
 * The follow-up the chosen threads become, in the shape of the run-changes review's notes: the header (`git.checks.followUp`,
 * "PR #42 ..."), then one bullet per thread in file and line order, `- path:line (author): comment`, a comment's further lines indented
 * under it. Capped at [`FollowUpLimit`] characters (cut with an ellipsis). None chosen makes no request.
 */
export function threadsFollowUp(header: string, threads: readonly ChecksThread[]): string {
  if (threads.length === 0) return '';
  const sorted = threads.map((thread, index) => ({ thread, index })).sort((a, b) => {
    if (a.thread.path !== b.thread.path) return a.thread.path < b.thread.path ? -1 : 1;
    return (a.thread.line ?? 0) - (b.thread.line ?? 0) || a.index - b.index;
  });
  const lines = [header.trim()];
  for (const { thread } of sorted) {
    const place = threadPlace(thread);
    const who = thread.author ? `(${thread.author})` : '';
    const lead = [place, who].filter((part) => part !== '').join(' ');
    const body = thread.body.replace(/\r\n/g, '\n').trim().split('\n').join('\n  ');
    lines.push('- ' + (lead === '' ? body : `${lead}: ${body}`));
  }
  const text = lines.join('\n');
  const chars = [...text];
  return chars.length > FollowUpLimit ? chars.slice(0, FollowUpLimit - 1).join('') + '…' : text;
}

/** The follow-up's header: a pull request's or a merge request's. */
export function followUpHeader(pr: Pick<ChecksPr, 'hosting' | 'number'>, t: Translate): string {
  return t(pr.hosting === 'gitlab' ? 'git.checks.followUpMr' : 'git.checks.followUp', { number: pr.number });
}

/** A part's state glyph: passed, a warning, or could not be read. */
export function partTone(ok: boolean, warns: boolean): { icon: IconName; tone: 'done' | 'err' | 'wait' } {
  if (!ok) return { icon: 'circle-question-mark', tone: 'wait' };
  return warns ? { icon: 'triangle-alert', tone: 'err' } : { icon: 'circle-check', tone: 'done' };
}

/** Whether a warning of `codes` is among the warnings. */
export function warns(warnings: readonly ChecksWarning[], ...codes: ChecksWarningCode[]): boolean {
  return warnings.some((warning) => codes.includes(warning.code as ChecksWarningCode));
}

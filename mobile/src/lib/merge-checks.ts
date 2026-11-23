import type { ChecksPart, MergeChecks, MergeWarning } from '@/api/types';
import { t } from '@/lib/i18n';

/**
 * MC-27, 머지 전 점검 on the phone: the desktop's 점검 tab (`gitWorkbenchChecks`, src-tauri merge_checks.rs) read for a worktree
 * pane through `GET /m1/sessions/{id}/checks`, drawn read-only, and the warnings a merge is asked about once more. The words are
 * the desktop's own keys (`git.checks.*`), so both say the same thing.
 */

/** A warning in words: `실패한 CI 검사 2개`. A code this build does not know reads as the remaining checklist's line. */
export function warningLine(warning: MergeWarning): string {
  const count = warning.count;
  switch (warning.code) {
    case 'uncommitted':
      return t('git.checks.warn.uncommitted', { count });
    case 'draft':
      return t('git.checks.warn.draft');
    case 'conflicts':
      return t('git.checks.warn.conflicts');
    case 'changesRequested':
      return t('git.checks.warn.changesRequested');
    case 'checksFailed':
      return t('git.checks.warn.checksFailed', { count });
    case 'checksPending':
      return t('git.checks.warn.checksPending', { count });
    case 'threads':
      return t('git.checks.warn.threads', { count });
    default:
      return t('git.checks.warn.checklist', { count });
  }
}

/** The warnings a host sent, read defensively: an entry without a code or a count is left out. */
export function parseWarnings(raw: unknown): MergeWarning[] {
  if (!Array.isArray(raw)) return [];
  return raw.filter(
    (entry): entry is MergeWarning =>
      typeof entry === 'object' && entry !== null && typeof entry.code === 'string' && typeof entry.count === 'number',
  );
}

/** The merge's second question: `병합 전에 확인하세요`, the desktop's sentence and one line a warning. */
export function mergeConfirmText(warnings: readonly MergeWarning[]): string {
  return [t('git.checks.merge.message'), ...warnings.map((warning) => `• ${warningLine(warning)}`)].join('\n');
}

/** Why a part could not be read, in words. */
function unknownReason(part: { reason: string; cli?: string }): string {
  switch (part.reason) {
    case 'noRemote':
      return t('git.pr.noRemote');
    case 'cliMissing':
      return part.cli === 'glab' ? t('git.pr.error.glabMissing') : t('git.pr.error.ghMissing');
    case 'notLoggedIn':
      return t('git.checks.reason.notLoggedIn', { cli: part.cli ?? 'gh' });
    case 'noPr':
      return t('git.pr.none');
    default:
      return t('git.checks.reason.failed');
  }
}

export type ChecksTone = 'done' | 'wait' | 'err';

/** One part of the checks as the sheet draws it: its title, a tone and its lines. */
export interface ChecksSection {
  id: 'git' | 'pr' | 'ci' | 'threads' | 'checklist';
  title: string;
  tone: ChecksTone;
  lines: string[];
}

function unknownSection(id: ChecksSection['id'], title: string, part: { reason: string; cli?: string }): ChecksSection {
  return { id, title, tone: 'wait', lines: [`${t('git.checks.unknown')} · ${unknownReason(part)}`] };
}

function read<T>(part: ChecksPart<T> | undefined): { value: T } | { unknown: { reason: string; cli?: string } } {
  if (part && part.ok === true) return { value: part.value };
  if (part && part.ok === false) return { unknown: { reason: part.reason, ...(part.cli ? { cli: part.cli } : {}) } };
  return { unknown: { reason: 'failed' } };
}

function prState(state: string): string {
  switch (state) {
    case 'open':
      return t('git.pr.state.open');
    case 'merged':
      return t('git.pr.state.merged');
    case 'closed':
      return t('git.pr.state.closed');
    default:
      return state;
  }
}

/** The most lines a list (failed checks, threads, checklist items) shows; the count says the rest. */
const LIST_LINES = 5;

/** The checks as the sheet's sections, in the 점검 tab's order. Each part stands alone: one not read says why. */
export function checksSections(model: MergeChecks): ChecksSection[] {
  const sections: ChecksSection[] = [];

  const git = read(model.git);
  const gitTitle = t('git.checks.git.title');
  if ('value' in git) {
    const value = git.value;
    const lines: string[] = [];
    lines.push(value.uncommitted > 0 ? t('git.checks.warn.uncommitted', { count: value.uncommitted }) : t('git.checks.git.clean'));
    if (value.conflicted > 0) lines.push(t('git.checks.git.conflicted', { count: value.conflicted }));
    if (value.base) lines.push(t('git.checks.git.base', { base: value.base, ahead: value.ahead, behind: value.behind }));
    sections.push({ id: 'git', title: gitTitle, tone: value.uncommitted > 0 || value.conflicted > 0 ? 'err' : 'done', lines });
  } else sections.push(unknownSection('git', gitTitle, git.unknown));

  const pr = read(model.pr);
  const prTitle = t('git.checks.pr.title');
  let open = false;
  if ('value' in pr) {
    const value = pr.value;
    open = value.state === 'open';
    const head = t('git.pr.line', { number: value.number, state: prState(value.state) });
    const lines = [value.isDraft ? `${head} · ${t('panePr.draft')}` : head];
    if (value.title) lines.push(value.title);
    let warns = open && value.isDraft;
    if (open && value.mergeable === 'conflicting') {
      lines.push(t('git.checks.pr.conflicts'));
      warns = true;
    }
    if (value.reviewDecision === 'approved') lines.push(t('git.checks.pr.approved'));
    else if (value.reviewDecision === 'changesRequested') {
      lines.push(t('git.checks.pr.changesRequested'));
      warns = warns || open;
    } else if (value.reviewDecision === 'reviewRequired') lines.push(t('git.checks.pr.reviewRequired'));
    sections.push({ id: 'pr', title: prTitle, tone: warns ? 'err' : 'done', lines });
  } else sections.push(unknownSection('pr', prTitle, pr.unknown));

  const ci = read(model.checks);
  const ciTitle = t('git.checks.ci.title');
  if ('value' in ci) {
    const items = Array.isArray(ci.value) ? ci.value : [];
    const passed = items.filter((item) => item.state === 'passed').length;
    const failed = items.filter((item) => item.state === 'failed');
    const pending = items.filter((item) => item.state === 'pending').length;
    const lines =
      items.length === 0
        ? [t('git.checks.ci.none')]
        : [t('git.checks.ci.summary', { passed, failed: failed.length, pending })];
    for (const item of failed.slice(0, LIST_LINES)) lines.push(`✕ ${item.name}`);
    const tone: ChecksTone = failed.length > 0 ? 'err' : pending > 0 ? 'wait' : 'done';
    sections.push({ id: 'ci', title: ciTitle, tone, lines });
  } else sections.push(unknownSection('ci', ciTitle, ci.unknown));

  const threads = read(model.threads);
  const threadsTitle = t('git.checks.threads.title');
  if ('value' in threads) {
    const list = Array.isArray(threads.value) ? threads.value : [];
    const lines = list.length === 0 ? [t('git.checks.threads.none')] : [t('git.checks.warn.threads', { count: list.length })];
    for (const thread of list.slice(0, LIST_LINES)) {
      const place = thread.path ? (thread.line && thread.line > 0 ? `${thread.path}:${thread.line}` : thread.path) : '';
      const who = thread.author ? `${thread.author}: ` : '';
      const body = thread.body.replace(/\s+/g, ' ').trim();
      const outdated = thread.outdated ? ` (${t('git.checks.threads.outdated')})` : '';
      lines.push(`${place ? `${place} — ` : ''}${who}${body}${outdated}`);
    }
    sections.push({ id: 'threads', title: threadsTitle, tone: list.length > 0 ? 'err' : 'done', lines });
  } else sections.push(unknownSection('threads', threadsTitle, threads.unknown));

  const checklist = read(model.checklist);
  const checklistTitle = t('git.checks.checklist.title');
  if ('value' in checklist) {
    const { total, remaining } = checklist.value;
    const items = Array.isArray(remaining) ? remaining : [];
    const lines =
      total === 0
        ? [t('git.checks.checklist.noList')]
        : items.length === 0
          ? [t('git.checks.checklist.none')]
          : [t('git.checks.warn.checklist', { count: items.length }), ...items.slice(0, LIST_LINES).map((item) => `☐ ${item}`)];
    sections.push({ id: 'checklist', title: checklistTitle, tone: items.length > 0 ? 'err' : 'done', lines });
  } else sections.push(unknownSection('checklist', checklistTitle, checklist.unknown));

  return sections;
}

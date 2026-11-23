import type { CompareColumn, MobileCompareChild, MobileCompareRun, MobileSessionSummary } from '@/api/types';
import { t } from '@/lib/i18n';
import { modelLabel } from '@/lib/model-label';
import type { Tone } from '@/lib/status-tone';

/**
 * MC-29, 비교 실행 on the phone: the compare runs a pane started (its summary's `compareRuns`) as a card under its bar, and the
 * desktop's compare view (`GET /m1/sessions/{id}/compare/{compareId}`, "compare") as a panel with each child's agent and model,
 * state, changes, time, cost and test result, with open, merge (reported children only, through the pre-merge checks) and discard,
 * and after a merge the offer to discard the rest. The words are the desktop's (`compare.*`).
 */

/** A compare run's first line of its request, for the card's subtitle. */
export function compareRequestLine(run: Pick<MobileCompareRun, 'request'>): string {
  return (run.request ?? '').split('\n').find((line) => line.trim().length > 0)?.trim() ?? '';
}

/** The compare runs a summary carries, read defensively (an entry without an id or children is left out), newest first. */
export function compareRunsOf(session: MobileSessionSummary | undefined): MobileCompareRun[] {
  const runs = session?.compareRuns;
  if (!Array.isArray(runs)) return [];
  return runs.filter((run) => typeof run?.id === 'string' && Array.isArray(run.children)).slice().reverse();
}

/** A phase in words. */
export function phaseWord(phase: string): string {
  switch (phase) {
    case 'starting':
      return t('compare.phase.starting');
    case 'running':
      return t('compare.phase.running');
    case 'reported':
      return t('compare.phase.reported');
    case 'ended':
      return t('compare.phase.ended');
    case 'failed':
      return t('compare.phase.failed');
    case 'merged':
      return t('compare.phase.merged');
    case 'discarded':
      return t('compare.phase.discarded');
    default:
      return phase;
  }
}

/** The tone a phase is drawn in, as the desktop's compare view draws it. */
export function phaseTone(phase: string): Tone {
  switch (phase) {
    case 'starting':
    case 'running':
      return 'run';
    case 'reported':
      return 'done';
    case 'failed':
      return 'err';
    case 'merged':
      return 'wait';
    default:
      return 'idle';
  }
}

/** Seconds as `m:ss`, or `h:mm:ss` past an hour; `—` when unknown. */
export function elapsedWords(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '—';
  const whole = Math.floor(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const rest = String(whole % 60).padStart(2, '0');
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, '0')}:${rest}` : `${minutes}:${rest}`;
}

/** A cost in dollars (`$0.42`); `—` when unknown. */
export function usd(value: number | null | undefined): string {
  return value === null || value === undefined || !Number.isFinite(value) ? '—' : `$${value.toFixed(2)}`;
}

/** A column's changes: `파일 3개 +12 −4`, `변경 없음`, or `—` while git could not say. */
export function changesWords(column: Pick<CompareColumn, 'files' | 'added' | 'removed'>): string {
  const files = column.files;
  if (files === null || files === undefined) return '—';
  if (files === 0) return t('compare.view.noChanges');
  return `${t('compare.view.files', { count: files })} +${column.added ?? 0} −${column.removed ?? 0}`;
}

/** A test result in words: none chosen, waiting for the report, running, passed, or failed with its code. */
export function testWords(state: string | undefined, exitCode: number | null | undefined, chosen: boolean): string {
  if (!chosen) return t('compare.test.none');
  if (!state) return t('compare.test.waiting');
  if (state === 'running') return t('compare.test.running');
  if (state === 'passed') return t('compare.test.passed');
  return t('compare.test.failed', { code: exitCode ?? '—' });
}

/** The agent and model a column ran: `Claude · Opus 5.5`, or the agent alone on its default model. */
export function columnAgent(column: Pick<CompareColumn, 'agent' | 'agentLabel' | 'model'>): string {
  const agent = column.agentLabel || column.agent;
  return column.model ? `${agent} · ${modelLabel(column.model)}` : `${agent} · ${t('compare.picker.defaultModel')}`;
}

/**
 * A column from the summary alone, for a host without the compare route (or before the view arrives): the child's phase is read
 * from its pane's sidebar state when the phone lists that pane, a failure to start fails it, and git's numbers are unknown.
 */
export function summaryColumn(child: MobileCompareChild, panes: readonly MobileSessionSummary[]): CompareColumn {
  const pane = panes.find((session) => session.id === child.id);
  return {
    id: child.id,
    agent: child.provider,
    model: child.model ?? null,
    phase: child.failure ? 'failed' : summaryPhase(pane?.childState),
    failure: child.failure ?? null,
    files: null,
    costUsd: child.costUsd ?? null,
    test: child.testState ? { state: child.testState } : null,
    canOpen: pane !== undefined,
    canMerge: false,
    canDiscard: false,
  };
}

/** A child pane's sidebar state as a compare phase; a child the phone does not list has ended for all it can tell. */
function summaryPhase(state: string | undefined): string {
  switch (state) {
    case 'creating':
      return 'starting';
    case 'running':
    case 'waiting':
      return 'running';
    case 'reported':
    case 'merged':
    case 'discarded':
      return state;
    default:
      return 'ended';
  }
}

/** The card's counts in the desktop sidebar card's words (`실행 중 1 · 보고 2`), from the columns' phases. */
export function phaseCounts(columns: readonly Pick<CompareColumn, 'phase'>[]): string {
  let running = 0;
  let reported = 0;
  let failed = 0;
  let merged = 0;
  let discarded = 0;
  for (const column of columns) {
    if (column.phase === 'starting' || column.phase === 'running') running += 1;
    else if (column.phase === 'reported') reported += 1;
    else if (column.phase === 'merged') merged += 1;
    else if (column.phase === 'discarded') discarded += 1;
    else failed += 1;
  }
  const parts: string[] = [];
  if (running > 0) parts.push(t('compare.card.running', { count: running }));
  if (reported > 0) parts.push(t('compare.card.reported', { count: reported }));
  if (failed > 0) parts.push(t('compare.card.failed', { count: failed }));
  if (merged > 0) parts.push(t('compare.card.merged', { count: merged }));
  if (discarded > 0) parts.push(t('compare.card.discarded', { count: discarded }));
  return parts.join(' · ');
}

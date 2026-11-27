// MC-119: pure rules for the loop sidebar tab, detail view, pane band, and create sheet.
import type { Translate } from '../i18n/locale';
import type { Criterion, CriterionCheck, CriterionLevel, LoopLimits, LoopRecord, LoopStartArgs } from './types';

export type LoopGroup = 'waiting' | 'running' | 'finished';
export const loopGroups: readonly LoopGroup[] = ['waiting', 'running', 'finished'];

export type LoopTone = 'wait' | 'run' | 'done' | 'err' | 'stop';

export const loopGroupKeys: Record<LoopGroup, string> = {
  waiting: 'loop.ui.group.waiting',
  running: 'loop.ui.group.running',
  finished: 'loop.ui.group.finished',
};

const knownStopKeys: Readonly<Record<string, string>> = {
  passed: 'loop.ui.stop.passed',
  maxIterations: 'loop.ui.stop.maxIterations',
  maxMinutes: 'loop.ui.stop.maxMinutes',
  maxCost: 'loop.ui.stop.maxCost',
  budget: 'loop.ui.stop.budget',
  sameFailure: 'loop.ui.stop.sameFailure',
  person: 'loop.ui.stop.person',
  repairEnded: 'loop.ui.stop.repairEnded',
  paneClosed: 'loop.ui.stop.paneClosed',
};

const knownGuardKeys: Readonly<Record<string, string>> = {
  testsRemoved: 'loop.ui.guard.testsRemoved',
  skipAdded: 'loop.ui.guard.skipAdded',
  protectedChanged: 'loop.ui.guard.protectedChanged',
};

const knownPhaseKeys: Readonly<Record<string, string>> = {
  build: 'loop.ui.phase.build',
  verify: 'loop.ui.phase.verify',
  repair: 'loop.ui.phase.repair',
  done: 'loop.ui.phase.done',
};

/** Whether a loop has reached a terminal state. */
export function isFinished(loop: Pick<LoopRecord, 'state'>): boolean {
  return loop.state === 'passed' || loop.state === 'failed' || loop.state === 'stopped';
}

/** Whether a loop has any anti-gaming guard flag raised. */
export function hasGuardFlags(loop: Pick<LoopRecord, 'guard'>): boolean {
  return (loop.guard?.flags?.length ?? 0) > 0;
}

/**
 * Which sidebar group a loop belongs to:
 * - finished: passed / failed / stopped
 * - waiting (나를 기다림): paused, or an active loop with any guard flag
 * - running (도는 중): running with a clean guard
 */
export function loopGroup(loop: Pick<LoopRecord, 'state' | 'guard'>): LoopGroup | null {
  if (loop.state === 'draft') return null;
  if (isFinished(loop)) return 'finished';
  if (loop.state === 'paused' || hasGuardFlags(loop)) return 'waiting';
  return 'running';
}

/** Loops grouped in order (waiting, running, finished), omitting empty groups. */
export function groupLoops(loops: readonly LoopRecord[]): { group: LoopGroup; loops: LoopRecord[] }[] {
  return loopGroups
    .map((group) => ({ group, loops: loops.filter((loop) => loopGroup(loop) === group) }))
    .filter((entry) => entry.loops.length > 0);
}

/** Counts per group for the sidebar tab badge and tooltip. */
export function loopCounts(loops: readonly LoopRecord[]): { waiting: number; running: number; finished: number; total: number } {
  let waiting = 0;
  let running = 0;
  let finished = 0;
  for (const loop of loops) {
    const g = loopGroup(loop);
    if (g === 'waiting') waiting += 1;
    else if (g === 'running') running += 1;
    else if (g === 'finished') finished += 1;
  }
  return { waiting, running, finished, total: waiting + running + finished };
}

/** Whether a loop matches the sidebar search query. */
export function loopMatches(loop: LoopRecord, search: string): boolean {
  const words = search.trim().toLowerCase();
  if (words.length === 0) return true;
  const hay = [
    loop.title,
    loop.goal,
    loop.stopReason ?? '',
    ...loop.criteria.flatMap((c) => [c.text, c.check.name ?? '', c.check.command ?? '']),
  ];
  return hay.some((part) => part.toLowerCase().includes(words));
}

/** The active loop (running or paused) driving a pane, derived from the loops list (at most one per pane). */
export function activeLoopForPane(loops: readonly LoopRecord[], paneId: string): LoopRecord | undefined {
  return loops.find((loop) => loop.paneId === paneId && (loop.state === 'running' || loop.state === 'paused'));
}

/** Current iteration number and configured maximum. */
export function iterationProgress(loop: Pick<LoopRecord, 'iterations' | 'limits'>): { current: number; max: number } {
  const current = loop.iterations.reduce((highest, item) => Math.max(highest, item.n), loop.iterations.length);
  return { current, max: Math.max(1, loop.limits.maxIterations) };
}

/** Clamped 0–1 share of a meter (iterations, cost, etc.). */
export function meterFraction(used: number, max: number): number {
  if (!(max > 0) || !Number.isFinite(used)) return 0;
  return Math.max(0, Math.min(1, used / max));
}

/** Formats spent USD with two decimals (`$1.20`). */
export function formatUsd(amount: number): string {
  const safe = Number.isFinite(amount) && amount >= 0 ? amount : 0;
  return '$' + safe.toFixed(2);
}

/** Formats a USD limit concisely (`$5` for whole dollars, `$5.50` otherwise). */
export function formatLimitUsd(amount: number): string {
  const safe = Number.isFinite(amount) && amount >= 0 ? amount : 0;
  return Number.isInteger(safe) ? '$' + String(safe) : '$' + safe.toFixed(2);
}

/** Whole elapsed minutes from `startedAt` to `finishedAt` (or `now`), clamped at 0. */
export function elapsedMinutes(startedAt: string, finishedAt?: string | null, now = Date.now()): number {
  const start = Date.parse(startedAt);
  if (Number.isNaN(start)) return 0;
  const end = finishedAt ? Date.parse(finishedAt) : now;
  if (Number.isNaN(end)) return 0;
  return Math.max(0, Math.floor((end - start) / 60_000));
}

/** Formats a minute count using `loop.ui.duration.*` keys. */
export function formatMinutes(minutes: number, t: Translate): string {
  const safe = Math.max(0, Math.floor(minutes));
  if (safe < 60) return t('loop.ui.duration.minutes', { n: safe });
  const hours = Math.floor(safe / 60);
  const rem = safe % 60;
  if (rem === 0) return t('loop.ui.duration.hours', { n: hours });
  return t('loop.ui.duration.hoursMinutes', { h: hours, m: rem });
}

/** Formats an iteration's duration in milliseconds or seconds/minutes. */
export function formatDurationMs(durationMs: number | null | undefined): string | null {
  if (typeof durationMs !== 'number' || !Number.isFinite(durationMs) || durationMs < 0) return null;
  if (durationMs < 10_000) return (Math.round(durationMs / 100) / 10).toFixed(1) + 's';
  const secs = Math.round(durationMs / 1000);
  if (secs < 60) return secs + 's';
  return `${Math.floor(secs / 60)}m ${String(secs % 60).padStart(2, '0')}s`;
}

/** First failing criterion of a loop, if any. */
export function firstFailingCriterion(loop: Pick<LoopRecord, 'criteria'>): Criterion | undefined {
  return loop.criteria.find((criterion) => criterion.status === 'failed');
}

/** Localized label for a stop reason (falls back to the raw host string when unknown). */
export function stopReasonWords(reason: string | null | undefined, t: Translate): string {
  if (!reason) return '';
  const key = knownStopKeys[reason];
  return key ? t(key) : reason;
}

/** Localized label for a guard flag (falls back to the raw host string when unknown). */
export function guardFlagWords(flag: string, t: Translate): string {
  const key = knownGuardKeys[flag];
  return key ? t(key) : flag;
}

/** Localized label for a loop phase. */
export function phaseWords(phase: string | null | undefined, t: Translate): string {
  if (!phase) return '';
  const key = knownPhaseKeys[phase];
  return key ? t(key) : phase;
}

/** State chip label key and visual tone for a loop. */
export function stateChip(loop: Pick<LoopRecord, 'state' | 'guard'>, t: Translate): { label: string; tone: LoopTone } {
  if (loop.state === 'passed') return { label: t('loop.ui.state.passed'), tone: 'done' };
  if (loop.state === 'failed') return { label: t('loop.ui.state.failed'), tone: 'err' };
  if (loop.state === 'stopped') return { label: t('loop.ui.state.stopped'), tone: 'stop' };
  if (loop.state === 'paused') return { label: t('loop.ui.state.paused'), tone: 'wait' };
  if (hasGuardFlags(loop)) return { label: t('loop.ui.state.guard'), tone: 'wait' };
  return { label: t('loop.ui.state.running'), tone: 'run' };
}

/** Resolves a criterion identifier from `iteration.failed` to a readable label. */
export function failedCriterionLabel(idOrName: string, criteria: readonly Criterion[]): string {
  const match = criteria.find((c) => c.id === idOrName || c.check.name === idOrName);
  return match ? (match.text || match.check.name || idOrName) : idOrName;
}

/** Short label for a criterion (prefers text, falls back to check name or id). */
export function criterionTitle(criterion: Criterion): string {
  return criterion.text.trim() || criterion.check.name?.trim() || criterion.id;
}

/**
 * One-line summary text for a sidebar loop row:
 * - iteration progress `n/max`
 * - contextual tail: guard flag, paused notice, first failing criterion, or stop reason
 */
export function loopRowText(loop: LoopRecord, t: Translate): { iterations: string; detail: string; cost: string } {
  const { current, max } = iterationProgress(loop);
  const iterations = t('loop.ui.iterationRatio', { n: current, max });
  const cost = `${formatUsd(loop.costUsd)} / ${formatLimitUsd(loop.limits.maxCostUsd)}`;

  if (isFinished(loop)) {
    const reason = loop.stopReason && loop.stopReason !== 'passed' ? stopReasonWords(loop.stopReason, t) : '';
    const detail = reason ? `${iterations} · ${reason}` : `${iterations} · ${formatUsd(loop.costUsd)}`;
    return { iterations, detail, cost };
  }

  if (hasGuardFlags(loop)) {
    const firstFlag = guardFlagWords(loop.guard.flags[0]!, t);
    return { iterations, detail: `${iterations} · ${firstFlag}`, cost };
  }

  if (loop.state === 'paused') {
    return { iterations, detail: `${iterations} · ${t('loop.ui.row.pausedHint')}`, cost };
  }

  const failing = firstFailingCriterion(loop);
  if (failing) {
    return { iterations, detail: `${iterations} · ${t('loop.ui.row.failing', { name: criterionTitle(failing) })}`, cost };
  }

  const phase = phaseWords(loop.phase, t);
  return { iterations, detail: phase ? `${iterations} · ${phase}` : iterations, cost };
}

// ── Create-sheet logic ───────────────────────────────────────────────────────

export interface DraftCriterion {
  id: string;
  enabled: boolean;
  text: string;
  check: CriterionCheck;
  level: CriterionLevel | string;
  custom?: boolean;
}

export interface LoopDraftForm {
  goal: string;
  criteria: DraftCriterion[];
  maxIterations: string;
  maxMinutes: string;
  maxCostUsd: string;
}

export const defaultLimits: LoopLimits = { maxIterations: 10, maxMinutes: 120, maxCostUsd: 5 };

/** Builds editable form state from a `loopDraft` response and the composer's text. */
export function formFromDraft(draft: Partial<LoopRecord> | null | undefined, composerGoal: string): LoopDraftForm {
  const limits = draft?.limits ?? defaultLimits;
  const criteria: DraftCriterion[] = (draft?.criteria ?? []).map((c, index) => ({
    id: c.id || `c${index + 1}`,
    enabled: true,
    text: c.text || c.check.name || c.id || `c${index + 1}`,
    check: { ...c.check },
    level: c.level || 'L2',
  }));
  const goal = composerGoal.trim().length > 0 ? composerGoal : (draft?.goal ?? '');
  return {
    goal,
    criteria,
    maxIterations: String(limits.maxIterations ?? defaultLimits.maxIterations),
    maxMinutes: String(limits.maxMinutes ?? defaultLimits.maxMinutes),
    maxCostUsd: String(limits.maxCostUsd ?? defaultLimits.maxCostUsd),
  };
}

/** Parses `name=command` into `{ name, command }`, splitting on the first `=`. Returns null when invalid. */
export function parseCommandCheck(raw: string): { name: string; command: string } | null {
  const eq = raw.indexOf('=');
  if (eq <= 0) return null;
  const name = raw.slice(0, eq).trim();
  const command = raw.slice(eq + 1).trim();
  if (name.length === 0 || command.length === 0) return null;
  return { name, command };
}

/** Appends a custom `command` criterion parsed from `name=command`. */
export function addCommandCriterion(
  form: LoopDraftForm,
  raw: string,
  level: CriterionLevel = 'L2',
): { ok: true; form: LoopDraftForm } | { ok: false; errorKey: string } {
  const parsed = parseCommandCheck(raw);
  if (!parsed) return { ok: false, errorKey: 'loop.ui.create.commandFormatError' };
  const used = new Set(form.criteria.map((c) => c.id));
  let seq = form.criteria.length + 1;
  while (used.has(`cmd-${seq}`)) seq += 1;
  const next: DraftCriterion = {
    id: `cmd-${seq}`,
    enabled: true,
    text: parsed.name,
    check: { kind: 'command', name: parsed.name, command: parsed.command, cwd: null },
    level,
    custom: true,
  };
  return { ok: true, form: { ...form, criteria: [...form.criteria, next] } };
}

/** Toggles whether a draft criterion is included in the loop. */
export function toggleDraftCriterion(form: LoopDraftForm, id: string): LoopDraftForm {
  return {
    ...form,
    criteria: form.criteria.map((c) => (c.id === id ? { ...c, enabled: !c.enabled } : c)),
  };
}

/** Edits a draft criterion's human-readable text. */
export function editDraftCriterionText(form: LoopDraftForm, id: string, text: string): LoopDraftForm {
  return {
    ...form,
    criteria: form.criteria.map((c) => (c.id === id ? { ...c, text } : c)),
  };
}

/** Removes a custom draft criterion from the form. */
export function removeDraftCriterion(form: LoopDraftForm, id: string): LoopDraftForm {
  return {
    ...form,
    criteria: form.criteria.filter((c) => c.id !== id),
  };
}

/** Whether a criterion satisfies the contract's requirement of at least one `command` or `verifyConfig` check. */
export function isExecutableCriterion(criterion: Pick<DraftCriterion, 'enabled' | 'check'>): boolean {
  return criterion.enabled && (criterion.check.kind === 'command' || criterion.check.kind === 'verifyConfig');
}

/** Validates the create-sheet form and builds the `loopStart` request payload. */
export function buildLoopStartArgs(
  paneId: string,
  form: LoopDraftForm,
): { ok: true; args: LoopStartArgs } | { ok: false; errorKey: string } {
  const goal = form.goal.trim();
  if (goal.length === 0) return { ok: false, errorKey: 'loop.ui.create.goalRequired' };

  const enabled = form.criteria.filter((c) => c.enabled);
  if (!enabled.some(isExecutableCriterion)) {
    return { ok: false, errorKey: 'loop.ui.create.commandRequired' };
  }

  const maxIterations = Number(form.maxIterations.trim());
  const maxMinutes = Number(form.maxMinutes.trim());
  const maxCostUsd = Number(form.maxCostUsd.trim());
  if (!Number.isInteger(maxIterations) || maxIterations < 1) {
    return { ok: false, errorKey: 'loop.ui.create.limitsInvalid' };
  }
  if (!Number.isInteger(maxMinutes) || maxMinutes < 1) {
    return { ok: false, errorKey: 'loop.ui.create.limitsInvalid' };
  }
  if (!Number.isFinite(maxCostUsd) || maxCostUsd <= 0) {
    return { ok: false, errorKey: 'loop.ui.create.limitsInvalid' };
  }

  const criteria: Criterion[] = enabled.map((c) => ({
    id: c.id,
    text: c.text.trim() || c.check.name || c.id,
    check: c.check,
    level: c.level,
    status: 'pending',
    evidence: null,
  }));

  return {
    ok: true,
    args: {
      paneId,
      goal,
      criteria,
      limits: { maxIterations, maxMinutes, maxCostUsd },
    },
  };
}

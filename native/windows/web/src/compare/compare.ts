// 비교 실행 (MC-29, src-tauri compare.rs): what the picker is offered (`compareOptions`), what a start sends (`compareStart`), the
// compare view's columns (`compareView`), and the small rules the page applies itself: the rows' first pick, the budget line (the
// same rough estimate the host checks again), the elapsed time and the cost as words.
import type { Translate } from '../i18n/locale';

export interface CompareModel { id: string; label: string }
export interface CompareAgent { agent: string; label: string; models: CompareModel[] }
export interface CompareBase { sha: string; short: string; subject: string; head: boolean }
export interface CompareTest { id: string; name: string; source: string; command: string }
export interface CompareEstimate { agent: string; model: string | null; usd: number }

/** `compareOptions`: what the picker offers, or why a compare run cannot start from the pane now. */
export interface CompareOptions {
  available: boolean;
  reason?: string;
  max?: number;
  parentAgent?: string;
  agents?: CompareAgent[];
  bases?: CompareBase[];
  branch?: string | null;
  tests?: CompareTest[];
  testsApproved?: boolean;
  budget?: { usd: number | null; spent: number; estimates: CompareEstimate[]; fallback: number };
}

/** One picked row: the agent and its model (null: the model a new pane of that agent would run). */
export interface CompareRowPick { agent: string; model: string | null }

/** `compareStart`'s answer. */
export interface CompareStartAnswer { ok: boolean; compareId?: string; started?: number; failed?: number; error?: string; budget?: string }

export type ComparePhase = 'starting' | 'running' | 'reported' | 'ended' | 'failed' | 'merged' | 'discarded';

/** One column of the compare view. */
export interface CompareColumn {
  id: string;
  agent: string;
  agentLabel: string;
  model: string | null;
  phase: ComparePhase;
  failure: string | null;
  files: number | null;
  added: number | null;
  removed: number | null;
  elapsed: number | null;
  running: boolean;
  costUsd: number | null;
  portBase: number | null;
  portCount: number | null;
  test: { state: 'running' | 'passed' | 'failed'; exitCode: number | null; output: string | null } | null;
  canOpen: boolean;
  canDiff: boolean;
  canMerge: boolean;
  canDiscard: boolean;
  branch: string | null;
}

/** `compareView`: the group and its columns; `gone` when it is no longer on its pane. */
export interface CompareView {
  gone?: boolean;
  id?: string;
  request?: string;
  baseCommit?: string;
  baseShort?: string;
  baseBranch?: string | null;
  test?: string | null;
  testName?: string | null;
  createdAt?: string | null;
  children?: CompareColumn[];
}

/** The rows the picker starts with: the parent's own agent and the next agent that can run (or the same one again: "2× Claude"). */
export function firstRows(options: CompareOptions): CompareRowPick[] {
  const agents = options.agents ?? [];
  const first = agents.find((a) => a.agent === options.parentAgent) ?? agents[0];
  if (!first || (options.max ?? 0) < 2) return [];
  const second = agents.find((a) => a.agent !== first.agent) ?? first;
  return [{ agent: first.agent, model: null }, { agent: second.agent, model: null }];
}

/** The row added by 행 추가: the agent least picked so far (the first one on a tie). */
export function nextRow(options: CompareOptions, rows: readonly CompareRowPick[]): CompareRowPick | null {
  const agents = options.agents ?? [];
  if (rows.length >= (options.max ?? 0)) return null;
  const count = (agent: string) => rows.filter((r) => r.agent === agent).length;
  let least: CompareAgent | undefined;
  for (const agent of agents) if (!least || count(agent.agent) < count(least.agent)) least = agent;
  return least ? { agent: least.agent, model: null } : null;
}

/** What one row is guessed to cost (the host's estimate for that agent and model, else for the agent, else the fallback). */
export function estimateFor(options: CompareOptions, row: CompareRowPick): number {
  const estimates = options.budget?.estimates ?? [];
  return estimates.find((e) => e.agent === row.agent && e.model === row.model)?.usd
    ?? estimates.find((e) => e.agent === row.agent && e.model === null)?.usd
    ?? options.budget?.fallback ?? 1;
}

/** The run's estimate, and the warning when a daily budget is set and today's spend plus the estimate would pass it. */
export function budgetCheck(options: CompareOptions, rows: readonly CompareRowPick[]): { estimate: number; over: boolean; spent: number; budget: number | null } {
  const estimate = rows.reduce((sum, row) => sum + estimateFor(options, row), 0);
  const spent = options.budget?.spent ?? 0;
  const budget = options.budget?.usd ?? null;
  return { estimate, spent, budget, over: budget !== null && budget > 0 && spent + estimate > budget };
}

export function usd(value: number): string {
  return '$' + value.toFixed(2);
}

/** Elapsed seconds as m:ss, or h:mm:ss past an hour. */
export function elapsedWords(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return '—';
  const whole = Math.floor(seconds);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** Each phase's word, named in full so the locale check sees them used. */
export const phaseKey: Record<ComparePhase, string> = {
  starting: 'compare.phase.starting', running: 'compare.phase.running', reported: 'compare.phase.reported', ended: 'compare.phase.ended',
  failed: 'compare.phase.failed', merged: 'compare.phase.merged', discarded: 'compare.phase.discarded',
};

/** The tone a phase is drawn in. */
export function phaseTone(phase: ComparePhase): 'run' | 'done' | 'err' | 'wait' | 'idle' {
  switch (phase) {
    case 'starting': case 'running': return 'run';
    case 'reported': return 'done';
    case 'failed': return 'err';
    case 'merged': return 'wait';
    default: return 'idle';
  }
}

/** A column's test result in words: none chosen, waiting for the report, running, passed, or failed with its code. */
export function testWords(column: CompareColumn, chosen: boolean, t: Translate): string {
  if (!chosen) return t('compare.test.none');
  const test = column.test;
  if (!test) return t('compare.test.waiting');
  if (test.state === 'running') return t('compare.test.running');
  if (test.state === 'passed') return t('compare.test.passed');
  return t('compare.test.failed', { code: test.exitCode ?? '—' });
}

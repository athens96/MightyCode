// MC-48: the verification receipts the host pushes (`verify`, src-tauri verify/mod.rs), and what a result card shows of them. Pure.
import type { Translate } from '../i18n/locale';

export type VerifyStatus = 'running' | 'repairing' | 'passed' | 'failed';
export type VerifyStopReason = 'maxRounds' | 'sameFailure' | 'budget' | 'person' | 'repairOff' | 'repairEnded';

export interface VerifyCommand {
  name: string;
  command: string;
  passed: boolean;
  exitCode?: number;
  timedOut?: boolean;
  durationMs: number;
  output?: string;
  error?: string;
}

export interface VerifyPage {
  url: string;
  status?: number;
  loaded?: boolean;
  consoleErrors?: string[];
  exceptions?: string[];
  failedRequests?: string[];
  accessibility?: string[];
  accessibilityNodes?: number;
  unnamedControls?: string[];
  screenshot?: string;
  error?: string;
}

export interface VerifyBrowserCounts {
  pages: number;
  consoleErrors: number;
  exceptions: number;
  failedRequests: number;
  unnamedControls: number;
  pageErrors: number;
}

export interface VerifyReceipt {
  id: string;
  status: VerifyStatus;
  rounds: number;
  repairs: number;
  maxRounds: number;
  stopReason?: VerifyStopReason;
  commands: VerifyCommand[];
  browser?: { browser?: string; pages: VerifyPage[]; unavailable?: string };
  browserCounts?: VerifyBrowserCounts;
  configHash: string;
  startedAt: string;
  finishedAt?: string;
  manual?: boolean;
  runIds: string[];
}

/** One pane's verification as the host pushed it (and `verifyState` answered: whether its workspace has an approved `verify`). */
export interface VerifyPane {
  paneId: string;
  autoRepair: boolean;
  running: boolean;
  receipts: VerifyReceipt[];
  configured?: boolean;
  approved?: boolean;
}

/** The host's push, and `verifyState`'s answer (which adds whether the workspace has an approved `verify`). */
export interface VerifyMessage {
  kind: 'verify';
  paneId: string;
  autoRepair: boolean;
  running: boolean;
  receipts: VerifyReceipt[];
  configured?: boolean;
  approved?: boolean;
}

/** The message laid over what the page knew (a push without the configuration words keeps the ones `verifyState` gave). */
export function applyVerify(before: VerifyPane | undefined, message: VerifyMessage): VerifyPane {
  const next: VerifyPane = { ...before, paneId: message.paneId, autoRepair: message.autoRepair, running: message.running, receipts: message.receipts };
  if (message.configured !== undefined) next.configured = message.configured;
  if (message.approved !== undefined) next.approved = message.approved;
  return next;
}

/** The receipt a request's result shows: the newest one whose last request is this run. */
export function receiptForRun(pane: VerifyPane | undefined, runId: string | undefined): VerifyReceipt | undefined {
  if (!pane || !runId) return undefined;
  return pane.receipts.find((receipt) => receipt.runIds.length > 0 && receipt.runIds[receipt.runIds.length - 1] === runId);
}

/** The receipt's title: 검증 통과, 검증 실패 (n회 시도), 검증 중, 수리 요청 중. */
export function receiptTitle(t: Translate, receipt: VerifyReceipt): string {
  switch (receipt.status) {
    case 'passed':
      return receipt.rounds > 1 ? t('verify.card.passedAfter', { rounds: String(receipt.rounds) }) : t('verify.card.passed');
    case 'failed':
      return t('verify.card.failed', { rounds: String(receipt.rounds) });
    case 'repairing':
      return t('verify.card.repairing', { round: String(receipt.repairs), max: String(receipt.maxRounds) });
    default:
      return t('verify.card.running');
  }
}

/** Why it stopped, in words; nothing for a pass or a run still going. */
export function stopWords(t: Translate, receipt: VerifyReceipt): string {
  return receipt.status === 'failed' && receipt.stopReason ? t('verify.stop.' + receipt.stopReason) : '';
}

/** A command's time: `0.4s`, `12s`, `2m 05s`. */
export function duration(ms: number): string {
  if (ms < 10_000) return (Math.round(ms / 100) / 10).toFixed(1) + 's';
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return seconds + 's';
  return Math.floor(seconds / 60) + 'm ' + String(seconds % 60).padStart(2, '0') + 's';
}

/** The browser's findings in one line, or why it did not run. */
export function browserLine(t: Translate, receipt: VerifyReceipt): string | null {
  if (!receipt.browser) return null;
  if (receipt.browser.unavailable) return t('verify.card.browserUnavailable', { reason: receipt.browser.unavailable });
  const c = receipt.browserCounts;
  if (!c) return null;
  return t('verify.card.browserCounts', {
    pages: String(c.pages),
    console: String(c.consoleErrors),
    exceptions: String(c.exceptions),
    requests: String(c.failedRequests),
    unnamed: String(c.unnamedControls),
  });
}

/** The screenshots of the receipt's pages, in order. */
export function screenshots(receipt: VerifyReceipt): string[] {
  return (receipt.browser?.pages ?? []).flatMap((page) => (page.screenshot ? [page.screenshot] : []));
}

/** The receipt's tone for its mark: `done`, `fail` or `run`. */
export function receiptTone(receipt: VerifyReceipt): 'done' | 'fail' | 'run' {
  return receipt.status === 'passed' ? 'done' : receipt.status === 'failed' ? 'fail' : 'run';
}

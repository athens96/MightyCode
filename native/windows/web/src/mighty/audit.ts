// MC-54: a request's audit (감사 기록): what one turn did, one line each in time order — the commands it ran, the files it edited and
// the files its checkpoint saw change, the addresses and web requests it opened, the approval cards and permission decisions, the
// sub-agents and child panes it delegated to, and its verification (MC-48). Made only from what the app already saved for the run
// (its blocks' entries, its checkpoints, the plan records, the pane's child panes and the verification receipt); nothing new is
// collected. Pure.
import type { AgentActivity, CheckpointFile, LogEntry, MightyGraphRun, PlanRecord, RunCheckpoint, RunSession } from '../store/models';
import type { Translate } from '../i18n/locale';
import { duration, receiptTitle, type VerifyReceipt } from './verify';

export type AuditKind = 'request' | 'command' | 'edit' | 'file' | 'web' | 'tool' | 'agent' | 'child' | 'approval' | 'verify' | 'result';
export type AuditState = 'done' | 'failed' | 'running' | 'waiting' | 'stopped' | 'denied' | 'info';

/** The kinds in the order the filter row lists them. */
export const AuditKinds: readonly AuditKind[] = ['request', 'command', 'edit', 'file', 'web', 'tool', 'agent', 'child', 'approval', 'verify', 'result'];

/** Each kind's word (its key written out, so the locale check finds it). */
export const KindKeys: Record<AuditKind, string> = {
  request: 'turnAudit.kind.request', command: 'turnAudit.kind.command', edit: 'turnAudit.kind.edit', file: 'turnAudit.kind.file', web: 'turnAudit.kind.web',
  tool: 'turnAudit.kind.tool', agent: 'turnAudit.kind.agent', child: 'turnAudit.kind.child', approval: 'turnAudit.kind.approval', verify: 'turnAudit.kind.verify',
  result: 'turnAudit.kind.result',
};
const StateKeys: Record<Exclude<AuditState, 'info'>, string> = {
  done: 'turnAudit.state.done', failed: 'turnAudit.state.failed', running: 'turnAudit.state.running', waiting: 'turnAudit.state.waiting',
  stopped: 'turnAudit.state.stopped', denied: 'turnAudit.state.denied',
};
const FileStatusKeys: Record<string, string> = {
  A: 'turnAudit.file.status.A', M: 'turnAudit.file.status.M', D: 'turnAudit.file.status.D', R: 'turnAudit.file.status.R', T: 'turnAudit.file.status.T',
};

export interface AuditRow {
  id: string;
  /** ISO 8601, when the app saw it; absent when the record kept no time (it then stands after the row before it). */
  at?: string;
  kind: AuditKind;
  state: AuditState;
  /** What: the command line, the path, the address, the name. */
  text: string;
  /** The text is code (a command, a path, an address): set in mono. */
  code: boolean;
  /** The tool's name as the agent called it, a verify command's name. */
  tool?: string;
  durationMs?: number;
  /** The sub-agent whose block it was recorded in; absent for the main session. */
  who?: string;
  /** More words: lines changed, a plan card's outcome, an exit code. */
  detail?: string;
}

/** What a copy said: copied (with how many values the host took out on the way), or why it could not. */
export type AuditCopied = { redacted: number } | { error: string };

export interface AuditCounts { commands: number; files: number; web: number; approvals: number; agents: number; tools: number }

export interface TurnAudit {
  runId: string;
  title: string;
  request: string;
  rows: AuditRow[];
  counts: AuditCounts;
  /** Reads and searches, left out of the rows. */
  reads: number;
  verify?: VerifyReceipt;
}

export interface AuditInput {
  run: MightyGraphRun;
  /** The request's title as its block says it ("요청 3 · Claude"). */
  title?: string;
  paneId: string;
  planHistory?: readonly PlanRecord[];
  receipt?: VerifyReceipt;
  /** Every pane: the ones started from this pane during the request are its child panes. */
  sessions?: readonly RunSession[];
  /** Now, in ms since 1970: the end of a request still running. */
  now?: number;
}

/** What a CLI writes back when a tool call was refused: Mighty Code's own denial, Claude's and the other agents' words. */
const Denied = /denied this tool request|permission to use .{1,80} (?:has been|was) denied|doesn't want to proceed|user (?:rejected|declined|denied)|rejected by the user|not allowed by the user/i;

const MaxFiles = 200;

function stateOf(state: string): AuditState {
  switch (state) {
    case 'completed': return 'done';
    case 'error': case 'failed': return 'failed';
    case 'running': return 'running';
    case 'waiting': return 'waiting';
    default: return 'stopped';
  }
}

/** A run's state: anything not ended (layout.ts `terminal`) still runs. */
function runState(status: string): AuditState {
  switch (status) {
    case 'completed': return 'done';
    case 'failed': case 'error': return 'failed';
    case 'stopped': case 'cancelled': case 'interrupted': return 'stopped';
    default: return 'running';
  }
}

/** A child pane's state: an idle pane has finished its work. */
function paneState(status: string): AuditState {
  return status === 'idle' ? 'done' : runState(status);
}

/** Where an activity goes: its own kind, a delegation, a terminal command or a browser among the MCP tools; null for what is left out. */
function kindOf(activity: AgentActivity): AuditKind | 'read' | null {
  const tool = (activity.toolName ?? '').toLowerCase();
  switch (activity.kind) {
    case 'command': return 'command';
    case 'edit': return 'edit';
    case 'web': return 'web';
    case 'agent': return 'agent';
    case 'read': case 'search': return 'read';
    case 'turn': return null;
    default:
      if (/delegat|spawn_agent|follow_up/.test(tool)) return 'agent';
      if (/run_in_terminal|run_command/.test(tool)) return 'command';
      if (/open_url|browser/.test(tool)) return 'web';
      return 'tool';
  }
}

const time = (at: string | undefined) => (at ? Date.parse(at) : NaN);

/** The request's first line. */
export function firstLine(text: string): string {
  return (text.split('\n').find((line) => line.trim() !== '') ?? '').trim();
}

/** The checkpoint whose files are the request's changes: its end, else the newest one that saw any. */
function changes(checkpoints: readonly RunCheckpoint[] | undefined): RunCheckpoint | undefined {
  const all = checkpoints ?? [];
  return all.find((c) => c.kind === 'end') ?? [...all].reverse().find((c) => c.files.length > 0);
}

function fileDetail(file: CheckpointFile, t: Translate): string {
  const parts = [t(FileStatusKeys[file.status] ?? FileStatusKeys.M!)];
  if (file.added !== undefined || file.removed !== undefined) parts.push(t('turnAudit.file.lines', { added: file.added ?? 0, removed: file.removed ?? 0 }));
  if (file.oldPath) parts.push(t('turnAudit.file.renamed', { from: file.oldPath }));
  return parts.join(' · ');
}

/** The audit of `run`, its rows in time order (a row without a time stays after the one before it). */
export function buildTurnAudit(input: AuditInput, t: Translate): TurnAudit {
  const { run } = input;
  const rows: AuditRow[] = [];
  const seen = new Set<string>();
  let reads = 0;
  const firstUser = run.rootEntries.find((entry) => entry.kind === 'user');
  const take = (entries: readonly LogEntry[], who?: string) => {
    for (const entry of entries) {
      const activity = entry.activity;
      if (!activity || seen.has(activity.id)) continue;
      seen.add(activity.id);
      const kind = kindOf(activity);
      if (kind === 'read') { reads += 1; continue; }
      if (kind === null) continue;
      const tool = activity.toolName?.trim() || undefined;
      const text = activity.summary.trim() || tool || '';
      const denied = activity.state === 'error' && Denied.test(activity.output ?? '');
      rows.push({
        id: 'activity:' + activity.id, at: entry.timestamp, kind: denied ? 'approval' : kind, state: denied ? 'denied' : stateOf(activity.state),
        text: denied && tool && text !== tool ? tool + ' · ' + text : text, code: kind !== 'agent' || denied, tool, who,
        ...(activity.durationMs !== undefined && Number.isFinite(activity.durationMs) ? { durationMs: activity.durationMs } : {}),      });
    }
  };
  take(run.rootEntries);
  for (const agent of run.agents) take(agent.entries, agent.title || undefined);
  take(run.resultEntries);

  // The window the request ran in: its request to its last record (now while it runs).
  const times = [...run.rootEntries, ...run.resultEntries, ...run.agents.flatMap((a) => a.entries)].map((e) => time(e.timestamp)).filter(Number.isFinite);
  const start = Number.isFinite(time(firstUser?.timestamp)) ? time(firstUser?.timestamp) : times.length ? Math.min(...times) : NaN;
  const running = runState(run.status) === 'running';
  const last = times.length ? Math.max(...times) : NaN;
  const end = running ? Math.max(input.now ?? Date.now(), Number.isFinite(last) ? last : 0) : last;

  const checkpoint = changes(run.checkpoints);
  if (checkpoint) {
    for (const file of checkpoint.files.slice(0, MaxFiles)) {
      rows.push({ id: 'file:' + file.path, at: checkpoint.at, kind: 'file', state: 'info', text: file.path, code: true, detail: fileDetail(file, t) });
    }
    const more = (checkpoint.more ?? 0) + Math.max(0, checkpoint.files.length - MaxFiles);
    if (more > 0) rows.push({ id: 'file:more', at: checkpoint.at, kind: 'file', state: 'info', text: t('turnAudit.more', { count: more }), code: false });
  }

  for (const record of input.planHistory ?? []) {
    if (record.graphRunId !== run.id && record.runId !== run.id) continue;
    const outcome = ['approvedAuto', 'approvedConfirm', 'revised'].includes(record.outcome) ? record.outcome : 'cancelled';
    rows.push({
      id: 'plan:' + record.id, at: record.decidedAt || record.receivedAt, kind: 'approval',
      state: outcome === 'cancelled' ? 'stopped' : outcome === 'revised' ? 'info' : 'done',
      text: t('turnAudit.plan', { outcome: t('plan.outcome.' + outcome) }), code: false,
    });
  }

  if (Number.isFinite(start)) {
    for (const child of input.sessions ?? []) {
      if (child.parentSessionId !== input.paneId) continue;
      const at = time(child.createdAt);
      if (!Number.isFinite(at) || at < start - 1000 || (Number.isFinite(end) && at > end + 5000)) continue;
      rows.push({
        id: 'child:' + child.id, at: child.createdAt, kind: 'child', state: paneState(child.status), text: child.title, code: false,
        detail: t('turnAudit.child', { provider: child.provider }),
      });
    }
  }

  const receipt = input.receipt;
  if (receipt) {
    for (const [index, command] of receipt.commands.entries()) {
      const detail = command.timedOut ? t('verify.card.timedOut') : command.exitCode !== undefined && !command.passed ? t('verify.card.exitCode', { code: command.exitCode }) : undefined;
      rows.push({
        id: 'verify:' + index, at: receipt.startedAt || undefined, kind: 'verify', state: command.passed ? 'done' : 'failed', text: command.command, code: true,
        tool: command.name, durationMs: command.durationMs, ...(detail ? { detail } : {}),
      });
    }
    rows.push({
      id: 'verify:receipt', at: receipt.finishedAt || receipt.startedAt || undefined, kind: 'verify',
      state: receipt.status === 'passed' ? 'done' : receipt.status === 'failed' ? 'failed' : 'running', text: receiptTitle(t, receipt), code: false,
    });
  }

  // Time order; a row without a time keeps its place after the row before it.
  let carried = Number.NEGATIVE_INFINITY;
  const keyed = rows.map((row, index) => {
    const at = time(row.at);
    if (Number.isFinite(at)) carried = at;
    return { row, index, key: Number.isFinite(at) ? at : carried };
  });
  keyed.sort((a, b) => a.key - b.key || a.index - b.index);
  const sorted = keyed.map((k) => k.row);

  const request = firstLine(run.input);
  sorted.unshift({ id: 'request', at: firstUser?.timestamp, kind: 'request', state: 'info', text: request, code: false });
  const state = runState(run.status);
  sorted.push({ id: 'result', at: Number.isFinite(last) && !running ? new Date(last).toISOString() : undefined, kind: 'result', state, text: state === 'info' ? '' : t(StateKeys[state]), code: false });

  const files = new Set(sorted.filter((r) => (r.kind === 'edit' || r.kind === 'file') && r.id !== 'file:more').map((r) => r.text));
  const counts: AuditCounts = {
    commands: sorted.filter((r) => r.kind === 'command').length,
    files: files.size,
    web: sorted.filter((r) => r.kind === 'web').length,
    approvals: sorted.filter((r) => r.kind === 'approval').length,
    agents: sorted.filter((r) => r.kind === 'agent' || r.kind === 'child').length,
    tools: sorted.filter((r) => r.kind === 'tool').length,
  };
  return { runId: run.id, title: input.title || request, request, rows: sorted, counts, reads, ...(receipt ? { verify: receipt } : {}) };
}

/** A row's clock, `14:03:27`, in the page's language; empty without a time. */
export function clock(at: string | undefined, language: string): string {
  const ms = time(at);
  if (!Number.isFinite(ms)) return '';
  return new Date(ms).toLocaleTimeString(language || 'en', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
}

/** The state's word; none for a row that only says something. */
export function stateWord(t: Translate, row: AuditRow): string {
  if (row.kind === 'request' || row.kind === 'result' || row.state === 'info') return '';
  return t(StateKeys[row.state]);
}

/** What follows a row's text: who, the tool, the state, the time it took, the detail. */
export function rowMeta(t: Translate, row: AuditRow): string[] {
  const parts: string[] = [];
  if (row.tool && row.tool !== row.text && !row.text.startsWith(row.tool + ' · ')) parts.push(row.tool);
  const word = stateWord(t, row);
  if (word) parts.push(word);
  if (row.durationMs !== undefined && row.durationMs >= 0) parts.push(duration(row.durationMs));
  if (row.detail) parts.push(row.detail);
  if (row.who) parts.push(t('turnAudit.who', { name: row.who }));
  return parts;
}

/** The counts in one line: 명령 5 · 파일 3 · 웹 1 · 승인 0 · 위임 1. */
export function summaryLine(t: Translate, audit: TurnAudit): string {
  const { commands, files, web, approvals, agents } = audit.counts;
  return t('turnAudit.summary', { commands, files, web, approvals, agents });
}

/** `text` as one Markdown code span: on one line, cut at `max`, fenced past any run of backticks it holds. */
export function codeSpan(text: string, max = 1000): string {
  let one = text.replace(/\s*\r?\n\s*/g, ' ⏎ ');
  if (one.length > max) one = one.slice(0, max) + '…';
  const longest = Math.max(0, ...(one.match(/`+/g) ?? []).map((run) => run.length));
  const fence = '`'.repeat(longest + 1);
  const pad = one.startsWith('`') || one.endsWith('`') ? ' ' : '';
  return fence + pad + one + pad + fence;
}

/** The audit as Markdown: a heading, the request, the counts, then one list item a row. */
export function auditMarkdown(t: Translate, audit: TurnAudit, language: string): string {
  const lines = ['## ' + t('turnAudit.title') + ' · ' + audit.title, ''];
  if (audit.request) lines.push('> ' + audit.request.replace(/\n/g, ' '), '');
  lines.push(summaryLine(t, audit) + (audit.reads > 0 ? ' · ' + t('turnAudit.reads', { count: audit.reads }) : ''), '');
  for (const row of audit.rows) {
    if (row.kind === 'request') continue;
    const at = clock(row.at, language);
    const what = row.code ? codeSpan(row.text) : row.text.replace(/\s*\r?\n\s*/g, ' ');
    const meta = rowMeta(t, row);
    lines.push('- ' + (at ? at + ' · ' : '') + '**' + t(KindKeys[row.kind]) + '** ' + what + (meta.length ? ' — ' + meta.join(' · ') : ''));
  }
  lines.push('', '_' + t('turnAudit.markdown.footer') + '_', '');
  return lines.join('\n');
}

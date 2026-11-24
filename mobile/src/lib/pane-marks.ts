import {
  isAgentIOPane,
  type MobileSandboxBlock,
  type MobileSessionDetail,
  type MobileSessionSummary,
  type MobileState,
} from '@/api/types';
import { t } from '@/lib/i18n';

/**
 * The marks the desktop puts on a pane that the phone shows beside it: MC-26's "waiting for you" (`attention`), MC-28's fork
 * link (`forkedFrom`) and MC-34's sandbox (`sandboxed`, `sandboxBlocked`). Every field is optional: an older host sends none,
 * and the phone then shows nothing more than before.
 */

/** The kind of a desktop mark in words (`승인 대기`, `벨`); a kind this build does not know reads as a notification. */
export function attentionKindWord(kind: string): string {
  switch (kind) {
    case 'bell':
      return t('attention.kind.bell');
    case 'approval':
      return t('attention.kind.approval');
    case 'question':
      return t('attention.kind.question');
    case 'setupFailed':
      return t('attention.kind.setupFailed');
    default:
      return t('attention.kind.notify');
  }
}

/** The mark a pane carries, when it is a well-formed one. */
export function attentionOfPane(session: MobileSessionSummary): MobileSessionSummary['attention'] {
  const mark = session.attention;
  return mark && typeof mark.kind === 'string' && typeof mark.text === 'string' ? mark : undefined;
}

/** The line a marked pane shows: its kind, then the desktop's words when they say more. */
export function attentionLine(session: MobileSessionSummary): string | undefined {
  const mark = attentionOfPane(session);
  if (!mark) return undefined;
  const kind = attentionKindWord(mark.kind);
  const text = mark.text.replace(/\s+/g, ' ').trim();
  return text && text !== kind ? `${kind} · ${text}` : kind;
}

/** A pane the user can open: an agent's own terminal is listed but stays on the desktop. */
function isUserPane(session: MobileSessionSummary): boolean {
  return !isAgentIOPane(session.kind);
}

/** 나를 기다리는 창: the panes of one host the desktop marks as waiting for you. */
export function countMarked(state: MobileState | undefined): number {
  if (!state) return 0;
  return state.sessions.filter((session) => isUserPane(session) && attentionOfPane(session) !== undefined).length;
}

/** The pane marked last across the hosts (where the dashboard's line goes), or nothing while none is marked. */
export function newestMarked(
  hosts: readonly { id: string }[],
  states: Readonly<Record<string, MobileState | undefined>>,
): { hostId: string; sessionId: string } | undefined {
  let best: { hostId: string; sessionId: string; at: number } | undefined;
  for (const host of hosts) {
    for (const session of states[host.id]?.sessions ?? []) {
      const mark = attentionOfPane(session);
      if (!mark || !isUserPane(session)) continue;
      const at = Date.parse(mark.at);
      const when = Number.isFinite(at) ? at : 0;
      if (!best || when > best.at) best = { hostId: host.id, sessionId: session.id, at: when };
    }
  }
  return best ? { hostId: best.hostId, sessionId: best.sessionId } : undefined;
}

/** The latest thing the pane's sandbox refused, in the desktop's words; nothing when it refused nothing. */
export function latestSandboxBlock(detail: Pick<MobileSessionDetail, 'sandboxBlocked'> | undefined): string | undefined {
  const list = detail?.sandboxBlocked;
  if (!Array.isArray(list) || list.length === 0) return undefined;
  const block: MobileSandboxBlock | undefined = list[list.length - 1];
  if (!block || typeof block.what !== 'string') return undefined;
  const reason = typeof block.reason === 'string' ? block.reason : '';
  return block.kind === 'tool'
    ? t('sandbox.review.denied', { what: block.what, reason })
    : t('sandbox.network.blocked', { what: block.what, reason });
}

/**
 * MC-48: the pane's newest verification in one line — 검증 통과, 검증 실패 (n회 시도) with why and the failing commands, 검증 중 —
 * and its tone; nothing without a receipt (or from an older host).
 */
export function verifyLine(
  detail: Pick<MobileSessionDetail, 'verify'> | undefined,
): { text: string; tone: 'done' | 'fail' | 'run' } | undefined {
  const receipt = detail?.verify;
  if (!receipt || typeof receipt.status !== 'string') return undefined;
  const rounds = String(typeof receipt.rounds === 'number' ? receipt.rounds : 0);
  if (receipt.status === 'passed') return { text: t('verify.phone.passed'), tone: 'done' };
  if (receipt.status !== 'failed') return { text: t('verify.phone.running'), tone: 'run' };
  const failing = (Array.isArray(receipt.commands) ? receipt.commands : []).filter((c) => c && c.passed === false).map((c) => c.name);
  const reason = typeof receipt.stopReason === 'string' ? t('verify.stop.' + receipt.stopReason) : '';
  const parts = [t('verify.phone.failed', { rounds }), failing.join(', '), reason].filter((part) => part !== '');
  return { text: parts.join(' · '), tone: 'fail' };
}

/** Where a forked pane came from: the source's id, its title (as the phone lists it now, else as it was), and whether it is listed. */
export function forkSource(
  session: MobileSessionSummary | undefined,
  sessions: readonly MobileSessionSummary[],
): { paneId: string; title: string; open: boolean } | undefined {
  const link = session?.forkedFrom;
  if (!link || typeof link.paneId !== 'string' || link.paneId.length === 0) return undefined;
  const source = sessions.find((other) => other.id === link.paneId);
  const title = source?.title || link.title || t('phone.card.untitled');
  return { paneId: link.paneId, title, open: source !== undefined };
}

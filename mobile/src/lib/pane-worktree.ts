import type { MobilePaneWorktree, MobileRunAction, MobileSessionSummary, PaneWorktreeAction } from '@/api/types';
import { t } from '@/lib/i18n';

/**
 * A pane that works in its own git worktree on the Mac (`MobilePaneWorktree`, docs/mobile-remote.md "창마다 worktree"):
 * what the phone says about it and which of the Mac's actions it offers. The Mac does the git work and refuses what does
 * not hold any more; these rules only keep the phone from offering what the Mac's bar would not.
 */

/** `worktree mighty/… → main`, the Mac bar's words. */
export function worktreeLabel(worktree: MobilePaneWorktree): string {
  return t('paneWorktree.bar.branch', { branch: worktree.branch, base: worktree.base });
}

/** The row's and the header's spoken line: `자기 worktree에서 일함: mighty/… → main`. */
export function worktreeMarker(worktree: MobilePaneWorktree): string {
  return t('paneWorktree.marker', { branch: worktree.branch, base: worktree.base });
}

/** `커밋 ↑2 · 기준 ↓0 · 변경 3`, once the Mac has read the counts; nothing before. */
export function worktreeCounts(worktree: MobilePaneWorktree): string | undefined {
  if (worktree.ahead === undefined && worktree.behind === undefined && worktree.changed === undefined) return undefined;
  return t('paneWorktree.list.counts', {
    ahead: worktree.ahead ?? 0,
    behind: worktree.behind ?? 0,
    changed: worktree.changed ?? 0,
  });
}

/** The setup command's state in the bar's words; nothing without one (or a word this build does not know). */
export function worktreeSetup(worktree: MobilePaneWorktree): { text: string; failed: boolean } | undefined {
  switch (worktree.setup) {
    case 'running':
      return { text: t('paneWorktree.bar.setupRunning'), failed: false };
    case 'done':
      return { text: t('paneWorktree.bar.setupDone'), failed: false };
    case 'failed':
      return { text: t('paneWorktree.bar.setupFailed'), failed: true };
    default:
      return undefined;
  }
}

export interface WorktreeOffer {
  action: PaneWorktreeAction;
  label: string;
  /** Asks first (discard). */
  confirm: boolean;
  /** Shown but not tappable, with the reason underneath. */
  disabledReason?: string;
}

/**
 * The bar's main button: **커밋하고 병합** while the worktree has changes not committed (the Mac would refuse a plain
 * merge), else **병합**. A conflict adds **충돌 해결 요청** before it.
 */
export function primaryOffers(worktree: MobilePaneWorktree): WorktreeOffer[] {
  const offers: WorktreeOffer[] = [];
  if (worktree.conflict) {
    offers.push({ action: 'resolveConflict', label: t('paneWorktree.bar.resolve'), confirm: false });
  }
  offers.push(
    (worktree.changed ?? 0) > 0
      ? { action: 'commitMerge', label: t('paneWorktree.bar.commitMerge'), confirm: false }
      : { action: 'merge', label: t('paneWorktree.bar.merge'), confirm: false },
  );
  return offers;
}

/** The bar's menu: the other merge, undo while the last merge can be undone, and discard (refused while children branch from the pane). */
export function menuOffers(worktree: MobilePaneWorktree): WorktreeOffer[] {
  const offers: WorktreeOffer[] = [];
  if ((worktree.changed ?? 0) > 0) offers.push({ action: 'merge', label: t('paneWorktree.bar.merge'), confirm: false });
  else offers.push({ action: 'commitMerge', label: t('paneWorktree.bar.commitMerge'), confirm: false });
  if (worktree.canUndo) {
    offers.push({ action: 'undo', label: t('paneWorktree.bar.undo', { base: worktree.base }), confirm: false });
  }
  const children = worktree.children ?? 0;
  offers.push({
    action: 'discard',
    label: t('paneWorktree.bar.discard'),
    confirm: true,
    ...(children > 0 ? { disabledReason: t('paneWorktree.refused.hasChildren', { count: children }) } : {}),
  });
  return offers;
}

/** The pane's worktree, from its summary; undefined for every other pane and for an older host. */
export function paneWorktreeOf(session: MobileSessionSummary | undefined): MobilePaneWorktree | undefined {
  return session?.worktree;
}

/** MC-25: the worktree's port block as the desktop's bar writes it (`:41010–41019`, one port `:41010`); nothing without one. */
export function worktreePortBlock(worktree: MobilePaneWorktree): string | undefined {
  const base = worktree.portBase ?? 0;
  const count = worktree.portCount ?? 0;
  if (!(base > 0) || !(count > 0)) return undefined;
  return count === 1 ? `:${base}` : `:${base}\u2013${base + count - 1}`;
}

/**
 * MC-25: the run actions running in the worktree, each with the address its output named — only an `http(s)` one, which is
 * the desktop's own (`localhost`), so the phone offers it to copy rather than to open. MC-44: with the same server at the
 * computer's Tailscale address (`privateUrl`) when the host sent one and it is one ([`isPrivateWebAddress`]); that one opens.
 */
export function worktreeActions(worktree: MobilePaneWorktree): MobileRunAction[] {
  if (!Array.isArray(worktree.actions)) return [];
  return worktree.actions
    .filter((action) => typeof action?.name === 'string' && action.name.length > 0)
    .map((action) => {
      const kept: MobileRunAction = { name: action.name };
      if (typeof action.url === 'string' && /^https?:\/\//i.test(action.url)) kept.url = action.url;
      if (typeof action.privateUrl === 'string' && isPrivateWebAddress(action.privateUrl)) kept.privateUrl = action.privateUrl;
      return kept;
    });
}

/**
 * MC-44: an `http://` address at one of Tailscale's private addresses (100.64.0.0/10, `fd7a:115c:a1e0::/48`) with a port — the
 * only kind of dev-server address the phone opens; anything else (a public address, another scheme, credentials) is not.
 */
export function isPrivateWebAddress(url: string): boolean {
  const match = /^http:\/\/(\[[0-9a-fA-F:]+\]|\d{1,3}(?:\.\d{1,3}){3}):(\d{1,5})(?:[/?#].*)?$/.exec(url);
  if (!match) return false;
  const port = Number(match[2]);
  if (!(port > 0 && port <= 65535)) return false;
  const host = match[1]!;
  if (host.startsWith('[')) return /^\[fd7a:115c:a1e0:/i.test(host);
  const parts = host.split('.').map(Number);
  if (parts.some((part) => part > 255)) return false;
  return parts[0] === 100 && parts[1]! >= 64 && parts[1]! <= 127;
}

/** MC-44: one row of the bar's run actions: a running one (멈춤, its address) or one the phone may start (시작). */
export interface RunActionRow {
  name: string;
  running: boolean;
  url?: string;
  privateUrl?: string;
}

/**
 * MC-44: the bar's run action rows: the running ones first (as [`worktreeActions`] keeps them), then the approved `.mighty.yaml`'s
 * others the host offers to start (`runActions`). Without the host's "runActions" only the running ones, shown as before.
 */
export function runActionRows(worktree: MobilePaneWorktree, canRun: boolean): RunActionRow[] {
  const running: RunActionRow[] = worktreeActions(worktree).map((action) => ({ ...action, running: true }));
  if (!canRun || !Array.isArray(worktree.runActions)) return running;
  const names = new Set(running.map((row) => row.name));
  const startable = worktree.runActions
    .filter((name): name is string => typeof name === 'string' && name.length > 0 && name.length <= 80 && !/[\u0000-\u001f\u007f]/.test(name))
    .filter((name) => {
      if (names.has(name)) return false;
      names.add(name);
      return true;
    })
    .map((name) => ({ name, running: false }));
  return [...running, ...startable];
}

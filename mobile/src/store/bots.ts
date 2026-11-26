import { create } from 'zustand';
import type { MobileBots } from '@/api/types';
import { onHostRemoved } from '@/store/hosts';

/** MC-107: what the phone holds of one host's bots. */
export interface HostBots {
  /** The last `GET /m1/bots`, and the `botsRevision` of `/m1/state` it was read for. */
  data?: MobileBots;
  revision?: number;
  loading: boolean;
  error?: string;
  /** The host has no bot routes (an older desktop app): the screen asks for an update instead. */
  unsupported: boolean;
}

export type BotsByHost = Record<string, HostBots>;

const EMPTY: HostBots = { loading: false, unsupported: false };

/** The bots read for `revision`: an answer for an older revision than the one held is dropped. */
export function applyBots(state: BotsByHost, hostId: string, data: MobileBots, revision: number | undefined): BotsByHost {
  const held = state[hostId] ?? EMPTY;
  if (revision !== undefined && held.revision !== undefined && revision < held.revision && held.data) return state;
  const next: HostBots = { data, loading: false, unsupported: false };
  if (revision !== undefined) next.revision = revision;
  return { ...state, [hostId]: next };
}

/** A read under way, failed, or refused by a host without the routes; what was held stays on screen. */
export function markBots(
  state: BotsByHost,
  hostId: string,
  change: { loading?: boolean; error?: string | undefined; unsupported?: boolean },
): BotsByHost {
  const held = state[hostId] ?? EMPTY;
  const next: HostBots = { ...held };
  if (change.loading !== undefined) next.loading = change.loading;
  if ('error' in change) {
    if (change.error === undefined) delete next.error;
    else next.error = change.error;
  }
  if (change.unsupported !== undefined) next.unsupported = change.unsupported;
  if (next.loading === held.loading && next.error === held.error && next.unsupported === held.unsupported) return state;
  return { ...state, [hostId]: next };
}

/** A report opened: read at once on the phone (the host's answer follows with the next read). */
export function readReport(state: BotsByHost, hostId: string, reportId: string): BotsByHost {
  const data = state[hostId]?.data;
  if (!data) return state;
  const report = data.reports.find((r) => r.id === reportId);
  if (!report || report.read) return state;
  const wasUnread = report.unread;
  const reports = data.reports.map((r) => (r.id === reportId ? { ...r, read: true, unread: false } : r));
  const bots = data.bots.map((bot) =>
    bot.report?.id === reportId ? { ...bot, report: { ...bot.report, unread: false } } : bot,
  );
  const unread = Math.max(0, data.unread - (wasUnread ? 1 : 0));
  return { ...state, [hostId]: { ...state[hostId]!, data: { ...data, reports, bots, unread } } };
}

/** An approval decided (or found gone): it leaves the list at once. */
export function dropApproval(state: BotsByHost, hostId: string, approvalId: string): BotsByHost {
  const data = state[hostId]?.data;
  if (!data || !data.approvals.some((a) => a.id === approvalId)) return state;
  const approvals = data.approvals.filter((a) => a.id !== approvalId);
  return { ...state, [hostId]: { ...state[hostId]!, data: { ...data, approvals } } };
}

/** A pairing removed: its bots go with it. */
export function forgetHost(state: BotsByHost, hostId: string): BotsByHost {
  if (!(hostId in state)) return state;
  const next = { ...state };
  delete next[hostId];
  return next;
}

interface BotsStore {
  hosts: BotsByHost;
  apply: (hostId: string, data: MobileBots, revision: number | undefined) => void;
  mark: (hostId: string, change: { loading?: boolean; error?: string | undefined; unsupported?: boolean }) => void;
  read: (hostId: string, reportId: string) => void;
  dropApproval: (hostId: string, approvalId: string) => void;
  clearHost: (hostId: string) => void;
}

export const useBotsStore = create<BotsStore>((set) => ({
  hosts: {},
  apply: (hostId, data, revision) => set((prev) => ({ hosts: applyBots(prev.hosts, hostId, data, revision) })),
  mark: (hostId, change) => set((prev) => ({ hosts: markBots(prev.hosts, hostId, change) })),
  read: (hostId, reportId) => set((prev) => ({ hosts: readReport(prev.hosts, hostId, reportId) })),
  dropApproval: (hostId, approvalId) => set((prev) => ({ hosts: dropApproval(prev.hosts, hostId, approvalId) })),
  clearHost: (hostId) => set((prev) => ({ hosts: forgetHost(prev.hosts, hostId) })),
}));

onHostRemoved((hostId) => useBotsStore.getState().clearHost(hostId));

/** The bots' refreshers per host (each poller registers one), so a screen can ask for a read now. */
const refreshers = new Map<string, Set<() => void>>();

export function registerBotsRefresh(hostId: string, refresh: () => void): () => void {
  const own = refreshers.get(hostId) ?? new Set();
  own.add(refresh);
  refreshers.set(hostId, own);
  return () => {
    own.delete(refresh);
    if (own.size === 0 && refreshers.get(hostId) === own) refreshers.delete(hostId);
  };
}

export function refreshBots(hostId: string | undefined): void {
  if (!hostId) return;
  // One read is enough: every poller of the host writes the same store.
  const first = refreshers.get(hostId)?.values().next().value;
  first?.();
}

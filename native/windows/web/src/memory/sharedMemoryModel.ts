// Pure types and state/validation helpers for the Shared Memory & Multi-Device Sync panel (`MC-13`).
import type { Translate } from '../i18n/locale';

export type SharedMemoryTab = 'project' | 'user' | 'conflicts' | 'settings';
export type SyncConnectionMode = 'online' | 'offline' | 'localOnly';
export type ConflictResolutionChoice = 'keepLocal' | 'keepRemote';

export interface MemoryScopeWire {
  kind: 'user' | 'project';
  workspaceId?: string;
  projectSlug?: string;
}

export interface SharedMemoryEntryWire {
  id: string;
  scope: MemoryScopeWire;
  key: string;
  title: string;
  content: string;
  tags: string[];
  autoInject: boolean;
  pinned: boolean;
  revision: number;
  seq: number;
  contentHash: string;
  updatedAt: string;
  updatedByDeviceId: string;
  updatedByDeviceName: string;
  deleted: boolean;
}

export interface MemoryConflictWire {
  id: string;
  entryId: string;
  key: string;
  scope: MemoryScopeWire;
  localVersion: SharedMemoryEntryWire;
  remoteVersion: SharedMemoryEntryWire;
  detectedAt: string;
}

export interface SharedMemorySettingsWire {
  shareProjectMemory: boolean;
  shareUserMemory: boolean;
  autoInject: boolean;
  customEndpointUrl?: string | null;
  hasEndpointToken?: boolean;
}

export interface RemoteMemoryHostStatus {
  connectionId: string;
  name: string;
  online: boolean;
  cached: boolean;
  cachedAt?: string | null;
  cursorSeq: number;
}

export interface SharedMemorySyncStatus {
  mode: SyncConnectionMode;
  connectedHosts: number;
  totalHosts: number;
  cachedHosts: number;
  lastSyncedAt?: string | null;
  hosts: RemoteMemoryHostStatus[];
}

export interface SharedMemoryStateResponse {
  workspaceId?: string | null;
  projectSlug?: string | null;
  deviceId: string;
  deviceName: string;
  serverSeq: number;
  projectEntries: SharedMemoryEntryWire[];
  userEntries: SharedMemoryEntryWire[];
  conflicts: MemoryConflictWire[];
  settings: SharedMemorySettingsWire;
  syncStatus: SharedMemorySyncStatus;
}

export interface SharedMemoryUpsertResponse {
  entry: SharedMemoryEntryWire;
  redactedCount: number;
  state: SharedMemoryStateResponse;
}

export interface MemoryDraft {
  id: string | null;
  key: string;
  title: string;
  content: string;
  tagsText: string;
  autoInject: boolean;
  pinned: boolean;
}

export function emptyMemoryDraft(): MemoryDraft {
  return {
    id: null,
    key: '',
    title: '',
    content: '',
    tagsText: '',
    autoInject: true,
    pinned: false,
  };
}

export function draftFromEntry(entry: SharedMemoryEntryWire): MemoryDraft {
  return {
    id: entry.id,
    key: entry.key,
    title: entry.title,
    content: entry.content,
    tagsText: formatTagInput(entry.tags),
    autoInject: entry.autoInject,
    pinned: entry.pinned,
  };
}

/** Splits comma- or space-separated tags into normalized unique lowercase slugs. */
export function parseTagInput(raw: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const piece of raw.split(/[,\s]+/)) {
    const cleaned = piece
      .trim()
      .replace(/^#+/, '')
      .toLowerCase()
      .replace(/[^a-z0-9._/-]/g, '');
    if (cleaned.length > 0 && !seen.has(cleaned)) {
      seen.add(cleaned);
      out.push(cleaned);
      if (out.length >= 16) break;
    }
  }
  return out;
}

export function formatTagInput(tags: readonly string[]): string {
  return tags.join(', ');
}

/** Sorts entries pinned-first, then newest `updatedAt`, then `key`, and filters by search query. */
export function filterMemoryEntries(
  entries: readonly SharedMemoryEntryWire[],
  query: string,
): SharedMemoryEntryWire[] {
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter((part) => part.length > 0);

  const visible = entries.filter((entry) => {
    if (entry.deleted) return false;
    if (terms.length === 0) return true;
    const haystack = [entry.key, entry.title, entry.content, entry.tags.join(' '), entry.updatedByDeviceName]
      .join(' ')
      .toLowerCase();
    return terms.every((term) => haystack.includes(term));
  });

  return [...visible].sort((a, b) => {
    if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
    if (a.updatedAt !== b.updatedAt) return b.updatedAt.localeCompare(a.updatedAt);
    return a.key.localeCompare(b.key);
  });
}

export function canSaveMemoryDraft(draft: MemoryDraft): boolean {
  const hasKeyChar = /[a-zA-Z0-9]/.test(draft.key.trim());
  return hasKeyChar && draft.content.trim().length > 0;
}

export function syncStatusLabel(status: SharedMemorySyncStatus | null | undefined, t: Translate): string {
  const mode = status?.mode ?? 'localOnly';
  if (mode === 'online') return t('sharedMemory.status.online');
  if (mode === 'offline') return t('sharedMemory.status.offline');
  return t('sharedMemory.status.localOnly');
}

export function syncStatusClass(status: SharedMemorySyncStatus | null | undefined): 'online' | 'offline' | 'local' {
  const mode = status?.mode ?? 'localOnly';
  if (mode === 'online') return 'online';
  if (mode === 'offline') return 'offline';
  return 'local';
}

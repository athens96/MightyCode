import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { lucide } from '../icons/lucide';
import { emptyState, Store } from '../store/store';
import { SharedMemoryPanel } from './SharedMemoryPanel';
import panelCssSource from './SharedMemoryPanel.css?raw';
import panelTsxSource from './SharedMemoryPanel.tsx?raw';
import modelTsSource from './sharedMemoryModel.ts?raw';
import {
  canSaveMemoryDraft,
  draftFromEntry,
  emptyMemoryDraft,
  filterMemoryEntries,
  parseTagInput,
  syncStatusClass,
  syncStatusLabel,
  type SharedMemoryEntryWire,
  type SharedMemoryStateResponse,
} from './sharedMemoryModel';

const ALLOWED_TRANSLATION_KEYS: Record<string, string> = {
  'dialog.close': 'Close',
  'sharedMemory.deleteConfirm': 'Click again to delete (also removed on synced devices)',
  'dialog.closeBusy': 'Busy',
  'dialog.resize': 'Resize',
  'dialog.loading': 'Loading...',
  'tooltip.dialog.doubleClickFit': 'Double-click to fit',
  'savedPrompts.save': 'Save',
  'savedPrompts.delete': 'Delete',
  'sharedMemory.open': 'Shared Memory',
  'sharedMemory.title': 'Shared Memory',
  'sharedMemory.subtitle': 'Incremental memory sync across paired hosts and workspaces',
  'sharedMemory.tab.project': 'Project',
  'sharedMemory.tab.user': 'User',
  'sharedMemory.tab.conflicts': 'Conflicts',
  'sharedMemory.tab.settings': 'Sync Settings',
  'sharedMemory.syncNow': 'Sync Now',
  'sharedMemory.syncing': 'Syncing...',
  'sharedMemory.status.online': 'Synced online',
  'sharedMemory.status.offline': 'Offline cache',
  'sharedMemory.status.localOnly': 'Local only',
  'sharedMemory.newEntry': 'New Memory',
  'sharedMemory.empty': 'No shared memory entries yet.',
  'sharedMemory.field.key': 'Key',
  'sharedMemory.field.title': 'Title',
  'sharedMemory.field.content': 'Memory Content',
  'sharedMemory.field.tags': 'Tags',
  'sharedMemory.autoInject': 'Auto-inject into agent runs',
  'sharedMemory.pinned': 'Pinned',
  'sharedMemory.redactedNotice': 'Redacted {count} secret(s) before saving.',
  'sharedMemory.conflict.keepLocal': 'Keep Local',
  'sharedMemory.conflict.keepRemote': 'Keep Remote',
  'sharedMemory.conflict.empty': 'No sync conflicts detected.',
  'sharedMemory.settings.shareProject': 'Share project memory with connected hosts',
  'sharedMemory.settings.shareUser': 'Share user global memory across hosts',
  'sharedMemory.settings.endpointUrl': 'Custom HTTPS Sync Endpoint',
  'sharedMemory.settings.endpointToken': 'Endpoint Bearer Token',
  'sharedMemory.settings.save': 'Save Sync Settings',
};

const testTranslator = translator([ALLOWED_TRANSLATION_KEYS]);

function makeEntry(overrides: Partial<SharedMemoryEntryWire> & { id: string; key: string }): SharedMemoryEntryWire {
  return {
    scope: { kind: 'project', workspaceId: 'ws-1', projectSlug: 'org/app' },
    title: overrides.key,
    content: 'Default memory body',
    tags: [],
    autoInject: true,
    pinned: false,
    revision: 1,
    seq: 1,
    contentHash: 'hash-1',
    updatedAt: '2026-11-22T10:00:00Z',
    updatedByDeviceId: 'dev-1',
    updatedByDeviceName: 'Studio Mac',
    deleted: false,
    ...overrides,
  };
}

describe('sharedMemoryModel', () => {
  it('normalizes tags, validates drafts, and sorts pinned entries ahead of unpinned ones', () => {
    expect(parseTagInput(' #Rust, Arch , rust, API_v2!! ')).toEqual(['rust', 'arch', 'api_v2']);

    const empty = emptyMemoryDraft();
    expect(canSaveMemoryDraft(empty)).toBe(false);
    expect(canSaveMemoryDraft({ ...empty, key: '---', content: 'body' })).toBe(false);
    expect(canSaveMemoryDraft({ ...empty, key: 'api-rules', content: 'Always validate DTOs' })).toBe(true);

    const olderPinned = makeEntry({
      id: '1',
      key: 'pinned-rule',
      title: 'Pinned Rule',
      content: 'Use design tokens everywhere',
      pinned: true,
      updatedAt: '2026-11-20T00:00:00Z',
    });
    const newerRegular = makeEntry({
      id: '2',
      key: 'recent-note',
      title: 'Recent Note',
      content: 'Check remote-cache fallback',
      pinned: false,
      tags: ['offline'],
      updatedAt: '2026-11-22T12:00:00Z',
    });
    const deletedEntry = makeEntry({
      id: '3',
      key: 'deleted-note',
      deleted: true,
    });

    const sorted = filterMemoryEntries([newerRegular, deletedEntry, olderPinned], '');
    expect(sorted.map((e) => e.key)).toEqual(['pinned-rule', 'recent-note']);

    const filtered = filterMemoryEntries([newerRegular, olderPinned], 'offline fallback');
    expect(filtered.map((e) => e.key)).toEqual(['recent-note']);

    const roundtripDraft = draftFromEntry(newerRegular);
    expect(roundtripDraft.tagsText).toBe('offline');
  });

  it('maps sync connection statuses to the localized labels and tone classes', () => {
    expect(
      syncStatusLabel({ mode: 'online', connectedHosts: 1, totalHosts: 1, cachedHosts: 1, hosts: [] }, testTranslator),
    ).toBe('Synced online');
    expect(
      syncStatusLabel({ mode: 'offline', connectedHosts: 0, totalHosts: 1, cachedHosts: 1, hosts: [] }, testTranslator),
    ).toBe('Offline cache');
    expect(syncStatusLabel(null, testTranslator)).toBe('Local only');

    expect(syncStatusClass({ mode: 'online', connectedHosts: 1, totalHosts: 1, cachedHosts: 1, hosts: [] })).toBe(
      'online',
    );
    expect(syncStatusClass({ mode: 'offline', connectedHosts: 0, totalHosts: 1, cachedHosts: 1, hosts: [] })).toBe(
      'offline',
    );
    expect(syncStatusClass(undefined)).toBe('local');
  });
});

describe('SharedMemoryPanel rendering and strict design/icon/locale guardrails', () => {
  it('renders nothing when closed and renders tabs, status, and form when open', () => {
    const store = new Store(emptyState);
    const mockState: SharedMemoryStateResponse = {
      workspaceId: 'ws-1',
      projectSlug: 'org/app',
      deviceId: 'dev-1',
      deviceName: 'MacBook',
      serverSeq: 2,
      projectEntries: [],
      userEntries: [],
      conflicts: [],
      settings: {
        shareProjectMemory: true,
        shareUserMemory: false,
        autoInject: true,
        customEndpointUrl: null,
      },
      syncStatus: {
        mode: 'localOnly',
        connectedHosts: 0,
        totalHosts: 0,
        cachedHosts: 0,
        hosts: [],
      },
    };
    const services = {
      bridge: { call: <T = unknown>(): Promise<T> => Promise.resolve(mockState as unknown as T) },
      store,
      t: testTranslator,
    } as unknown as AppServices;

    const closedHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(SharedMemoryPanel, { open: false, onClose: () => {} }),
      ),
    );
    expect(closedHtml).toBe('');

    const openHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(SharedMemoryPanel, { open: true, workspaceId: 'ws-1', onClose: () => {} }),
      ),
    );
    expect(openHtml).toContain('data-testid="shared-memory-panel"');
    expect(openHtml).toContain('Shared Memory');
    expect(openHtml).toContain('No shared memory entries yet.');
    expect(openHtml).toContain('Local only');
    expect(openHtml).toContain('Sync Now');
  });

  it('contains zero raw color literals, zero hardcoded Hangul, only registered Lucide icons, and only allowed i18n keys', () => {
    const sources: Array<[string, string]> = [
      ['sharedMemoryModel.ts', modelTsSource],
      ['SharedMemoryPanel.tsx', panelTsxSource],
      ['SharedMemoryPanel.css', panelCssSource],
    ];
    const rawColorPattern = /#[0-9a-fA-F]{3,8}\b|\b(?:rgb|rgba|hsl|hsla)\s*\(/;
    const hangulPattern = /[\uAC00-\uD7A3\u3131-\u318E]/;

    for (const [name, source] of sources) {
      expect(source, `${name} must not contain raw color literals`).not.toMatch(rawColorPattern);
      expect(source, `${name} must not contain hardcoded Hangul`).not.toMatch(hangulPattern);
    }

    // Check every `<Icon name="..."` literal in SharedMemoryPanel.tsx exists in `lucide.ts`.
    const iconMatches = [...panelTsxSource.matchAll(/name=(?:\{[^}]*\?\s*)?['"]([a-z0-9-]+)['"]/g)];
    for (const match of iconMatches) {
      const iconName = match[1]!;
      expect(iconName in lucide, `Icon "${iconName}" must exist in lucide.ts`).toBe(true);
    }

    // Check every `t('...')` call in SharedMemoryPanel.tsx & sharedMemoryModel.ts uses only allowed keys.
    const combinedTs = `${modelTsSource}\n${panelTsxSource}`;
    const keyMatches = [...combinedTs.matchAll(/\bt\(\s*['"]([a-zA-Z0-9_.]+)['"]/g)];
    expect(keyMatches.length).toBeGreaterThan(15);
    for (const match of keyMatches) {
      const key = match[1]!;
      expect(key in ALLOWED_TRANSLATION_KEYS, `Locale key "${key}" must be in allowed catalogue`).toBe(true);
    }
  });
});

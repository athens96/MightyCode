// Shared Memory & Multi-Device Sync panel (`MC-13`).
// Manages project-scoped and user-scoped agent memory entries, secret redaction notices,
// multi-device conflict resolution, `remote-cache` offline fallback status, and sharing settings.
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import {
  canSaveMemoryDraft,
  draftFromEntry,
  emptyMemoryDraft,
  filterMemoryEntries,
  parseTagInput,
  syncStatusClass,
  syncStatusLabel,
  type ConflictResolutionChoice,
  type MemoryConflictWire,
  type MemoryDraft,
  type SharedMemoryEntryWire,
  type SharedMemoryStateResponse,
  type SharedMemoryTab,
  type SharedMemoryUpsertResponse,
} from './sharedMemoryModel';
import './SharedMemoryPanel.css';

export interface SharedMemoryPanelProps {
  open: boolean;
  workspaceId?: string | null;
  onClose: () => void;
}

function renderInBodyPortal(node: ReactNode) {
  if (typeof document !== 'undefined' && document.body) {
    return createPortal(node, document.body);
  }
  return node;
}

export function SharedMemoryPanel({
  open,
  workspaceId: propWorkspaceId,
  onClose,
}: SharedMemoryPanelProps) {
  const { bridge, t } = useServices();
  usePageOverlay(open);

  const fallbackWorkspaceId = useStoreSelector((state) => state.snapshot?.activeWorkspaceId ?? null);
  const workspaceId = propWorkspaceId !== undefined ? propWorkspaceId : fallbackWorkspaceId;

  const [tab, setTab] = useState<SharedMemoryTab>('project');
  const [data, setData] = useState<SharedMemoryStateResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [acting, setActing] = useState(false);
  const [deleteArmed, setDeleteArmed] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [redactedCount, setRedactedCount] = useState(0);
  const [query, setQuery] = useState('');
  const [draft, setDraft] = useState<MemoryDraft>(() => emptyMemoryDraft());

  // Settings form state
  const [shareProject, setShareProject] = useState(true);
  const [shareUser, setShareUser] = useState(false);
  const [autoInjectGlobal, setAutoInjectGlobal] = useState(true);
  const [endpointUrl, setEndpointUrl] = useState('');
  const [endpointToken, setEndpointToken] = useState('');

  const applyState = useCallback((next: SharedMemoryStateResponse | null | undefined) => {
    if (!next) return;
    setData(next);
    setShareProject(next.settings.shareProjectMemory);
    setShareUser(next.settings.shareUserMemory);
    setAutoInjectGlobal(next.settings.autoInject);
    setEndpointUrl(next.settings.customEndpointUrl ?? '');
  }, []);

  const loadState = useCallback(
    async (showSpinner: boolean) => {
      if (showSpinner) setLoading(true);
      try {
        const res = await bridge.call<SharedMemoryStateResponse>('sharedMemoryState', { workspaceId });
        applyState(res);
        setError(null);
      } catch (reason: unknown) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        if (showSpinner) setLoading(false);
      }
    },
    [bridge, workspaceId, applyState],
  );

  useEffect(() => {
    if (!open) return undefined;
    let active = true;
    setLoading(true);
    setRedactedCount(0);
    void bridge
      .call<SharedMemoryStateResponse>('sharedMemoryState', { workspaceId })
      .then(
        (res) => {
          if (!active) return;
          applyState(res);
          setError(null);
          setLoading(false);
        },
        (reason: unknown) => {
          if (!active) return;
          setError(reason instanceof Error ? reason.message : String(reason));
          setLoading(false);
        },
      );
    return () => {
      active = false;
    };
  }, [open, workspaceId, bridge, applyState]);

  const rawEntries: readonly SharedMemoryEntryWire[] = useMemo(() => {
    if (!data) return [];
    return tab === 'user' ? data.userEntries : data.projectEntries;
  }, [data, tab]);

  const visibleEntries = useMemo(() => filterMemoryEntries(rawEntries, query), [rawEntries, query]);
  const conflicts: readonly MemoryConflictWire[] = data?.conflicts ?? [];

  if (!open) return null;

  const handleSelectTab = (nextTab: SharedMemoryTab) => {
    setTab(nextTab);
    setRedactedCount(0);
    if (nextTab === 'project' || nextTab === 'user') {
      setDraft(emptyMemoryDraft());
    }
  };

  const handleNewEntry = () => {
    setRedactedCount(0);
    setDraft(emptyMemoryDraft());
  };

  const handleEditEntry = (entry: SharedMemoryEntryWire) => {
    setRedactedCount(0);
    setDraft(draftFromEntry(entry));
  };

  const handleSaveEntry = async () => {
    if (acting || !canSaveMemoryDraft(draft)) return;
    setActing(true);
    setError(null);
    try {
      const scope = tab === 'user' ? 'user' : 'project';
      const res = await bridge.call<SharedMemoryUpsertResponse>('sharedMemoryUpsert', {
        id: draft.id,
        scope,
        workspaceId,
        key: draft.key.trim(),
        title: draft.title.trim(),
        content: draft.content,
        tags: parseTagInput(draft.tagsText),
        autoInject: draft.autoInject,
        pinned: draft.pinned,
      });
      if (res?.state) applyState(res.state);
      else await loadState(false);
      setRedactedCount(res?.redactedCount ?? 0);
      if (res?.entry) {
        setDraft(draftFromEntry(res.entry));
      }
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const handleDeleteEntry = async (entryId: string) => {
    if (acting) return;
    setActing(true);
    setError(null);
    try {
      const res = await bridge.call<{ ok: boolean; state?: SharedMemoryStateResponse }>('sharedMemoryDelete', {
        id: entryId,
        workspaceId,
      });
      if (res?.state) applyState(res.state);
      else await loadState(false);
      if (draft.id === entryId) {
        setDraft(emptyMemoryDraft());
      }
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const handleSyncNow = async () => {
    if (syncing || acting) return;
    setSyncing(true);
    setError(null);
    try {
      const res = await bridge.call<{ state?: SharedMemoryStateResponse }>('sharedMemorySync', { workspaceId });
      if (res?.state) applyState(res.state);
      else await loadState(false);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSyncing(false);
    }
  };

  const handleResolveConflict = async (conflictId: string, resolution: ConflictResolutionChoice) => {
    if (acting) return;
    setActing(true);
    setError(null);
    try {
      const res = await bridge.call<{ state?: SharedMemoryStateResponse }>('sharedMemoryResolveConflict', {
        conflictId,
        resolution,
        workspaceId,
      });
      if (res?.state) applyState(res.state);
      else await loadState(false);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const handleSaveSettings = async () => {
    if (acting) return;
    setActing(true);
    setError(null);
    try {
      const res = await bridge.call<{ state?: SharedMemoryStateResponse }>('sharedMemoryConfigure', {
        workspaceId,
        shareProjectMemory: shareProject,
        shareUserMemory: shareUser,
        autoInject: autoInjectGlobal,
        customEndpointUrl: endpointUrl.trim() || null,
        endpointToken: endpointToken.trim() ? endpointToken.trim() : undefined,
      });
      if (res?.state) applyState(res.state);
      else await loadState(false);
      setEndpointToken('');
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const statusTone = syncStatusClass(data?.syncStatus);
  const statusWords = syncStatusLabel(data?.syncStatus, t);

  const headerTools = (
    <div className="shared-memory-header-tools">
      <span
        className={'shared-memory-status-pill ' + statusTone}
        data-testid="shared-memory-sync-status"
      >
        <Icon name={statusTone === 'online' ? 'globe' : statusTone === 'offline' ? 'triangle-alert' : 'shield'} size={12} weight={2} />
        <span>{statusWords}</span>
      </span>
      <button
        type="button"
        className="btn small"
        data-testid="shared-memory-sync-now"
        disabled={syncing || loading}
        onClick={() => {
          void handleSyncNow();
        }}
      >
        <Icon
          name={syncing ? 'loader-circle' : 'refresh-cw'}
          size={12}
          weight={2}
          className={syncing ? 'shared-memory-spin' : undefined}
        />
        <span>{syncing ? t('sharedMemory.syncing') : t('sharedMemory.syncNow')}</span>
      </button>
    </div>
  );

  const dialog = (
    <DialogShell
      kind="panel"
      panel={'sharedMemory' as never}
      width={880}
      height={580}
      minWidth={600}
      minHeight={400}
      label={t('sharedMemory.title')}
      title={t('sharedMemory.title')}
      subtitle={t('sharedMemory.subtitle')}
      icon={<Icon name="brain-circuit" size={18} weight={1.8} />}
      tools={headerTools}
      onCancel={onClose}
      testId="shared-memory-panel"
      closeTestId="shared-memory-close"
      bodyClassName="shared-memory-body"
      backdrop={{ 'data-page-overlay': '' }}
      status={
        error ? (
          <span className="tone-text-err" data-testid="shared-memory-error">
            {error}
          </span>
        ) : undefined
      }
    >
      <nav className="shared-memory-tabs" role="tablist" aria-label={t('sharedMemory.title')}>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'project'}
          className={'shared-memory-tab' + (tab === 'project' ? ' active' : '')}
          data-testid="shared-memory-tab-project"
          onClick={() => handleSelectTab('project')}
        >
          <Icon name="notebook-text" size={13} weight={2} />
          <span>{t('sharedMemory.tab.project')}</span>
          {data && <span className="shared-memory-count">{data.projectEntries.length}</span>}
        </button>

        <button
          type="button"
          role="tab"
          aria-selected={tab === 'user'}
          className={'shared-memory-tab' + (tab === 'user' ? ' active' : '')}
          data-testid="shared-memory-tab-user"
          onClick={() => handleSelectTab('user')}
        >
          <Icon name="globe" size={13} weight={2} />
          <span>{t('sharedMemory.tab.user')}</span>
          {data && <span className="shared-memory-count">{data.userEntries.length}</span>}
        </button>

        <button
          type="button"
          role="tab"
          aria-selected={tab === 'conflicts'}
          className={'shared-memory-tab' + (tab === 'conflicts' ? ' active' : '')}
          data-testid="shared-memory-tab-conflicts"
          onClick={() => handleSelectTab('conflicts')}
        >
          <Icon name="triangle-alert" size={13} weight={2} />
          <span>{t('sharedMemory.tab.conflicts')}</span>
          {conflicts.length > 0 && (
            <span className="shared-memory-count warning" data-testid="shared-memory-conflict-count">
              {conflicts.length}
            </span>
          )}
        </button>

        <button
          type="button"
          role="tab"
          aria-selected={tab === 'settings'}
          className={'shared-memory-tab' + (tab === 'settings' ? ' active' : '')}
          data-testid="shared-memory-tab-settings"
          onClick={() => handleSelectTab('settings')}
        >
          <Icon name="share-2" size={13} weight={2} />
          <span>{t('sharedMemory.tab.settings')}</span>
        </button>
      </nav>

      {redactedCount > 0 && (
        <div className="shared-memory-redacted-banner" role="status" data-testid="shared-memory-redacted">
          <Icon name="shield" size={14} weight={2} />
          <span>{t('sharedMemory.redactedNotice', { count: redactedCount })}</span>
        </div>
      )}

      {(tab === 'project' || tab === 'user') && (
        <div className="shared-memory-columns">
          <aside className="shared-memory-list-pane">
            <div className="shared-memory-list-head">
              <div className="shared-memory-search">
                <Icon name="search" size={12} weight={2} />
                <input
                  type="search"
                  className="shared-memory-search-input"
                  data-testid="shared-memory-filter"
                  value={query}
                  aria-label={t('sharedMemory.title')}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </div>
              <button
                type="button"
                className="btn small primary"
                data-testid="shared-memory-new"
                onClick={handleNewEntry}
              >
                <Icon name="plus" size={12} weight={2.2} />
                <span>{t('sharedMemory.newEntry')}</span>
              </button>
            </div>

            <div className="shared-memory-list" role="listbox" aria-label={t('sharedMemory.title')}>
              {loading && !data ? (
                <DialogLoading rows={3} />
              ) : visibleEntries.length === 0 ? (
                <div className="shared-memory-empty" data-testid="shared-memory-empty">
                  {t('sharedMemory.empty')}
                </div>
              ) : (
                visibleEntries.map((entry) => {
                  const selected = draft.id === entry.id;
                  return (
                    <div
                      key={entry.id}
                      role="option"
                      aria-selected={selected}
                      tabIndex={0}
                      className={'shared-memory-row' + (selected ? ' selected' : '')}
                      data-testid={'shared-memory-row-' + entry.key}
                      onClick={() => handleEditEntry(entry)}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter' || event.key === ' ') {
                          event.preventDefault();
                          handleEditEntry(entry);
                        }
                      }}
                    >
                      <div className="shared-memory-row-top">
                        <span className="shared-memory-row-title">{entry.title}</span>
                        <div className="shared-memory-row-badges">
                          {entry.pinned && (
                            <span className="shared-memory-badge pinned" title={t('sharedMemory.pinned')}>
                              <Icon name="pin" size={10} weight={2.2} />
                            </span>
                          )}
                          {entry.autoInject && (
                            <span className="shared-memory-badge inject" title={t('sharedMemory.autoInject')}>
                              <Icon name="check" size={10} weight={2.2} />
                            </span>
                          )}
                        </div>
                      </div>
                      <div className="shared-memory-row-key mono">{entry.key}</div>
                      <div className="shared-memory-row-preview">{entry.content}</div>
                      {entry.tags.length > 0 && (
                        <div className="shared-memory-row-tags">
                          {entry.tags.map((tag) => (
                            <span key={tag} className="shared-memory-tag mono">
                              {tag}
                            </span>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })
              )}
            </div>
          </aside>

          <section className="shared-memory-editor">
            <div className="shared-memory-field-grid">
              <label className="shared-memory-field">
                <span className="shared-memory-label">{t('sharedMemory.field.key')}</span>
                <input
                  type="text"
                  className="shared-memory-input mono"
                  data-testid="shared-memory-input-key"
                  value={draft.key}
                  onChange={(event) => setDraft((prev) => ({ ...prev, key: event.target.value }))}
                />
              </label>

              <label className="shared-memory-field">
                <span className="shared-memory-label">{t('sharedMemory.field.title')}</span>
                <input
                  type="text"
                  className="shared-memory-input"
                  data-testid="shared-memory-input-title"
                  value={draft.title}
                  onChange={(event) => setDraft((prev) => ({ ...prev, title: event.target.value }))}
                />
              </label>
            </div>

            <label className="shared-memory-field shared-memory-content-field">
              <span className="shared-memory-label">{t('sharedMemory.field.content')}</span>
              <textarea
                className="shared-memory-textarea mono"
                data-testid="shared-memory-input-content"
                value={draft.content}
                onChange={(event) => setDraft((prev) => ({ ...prev, content: event.target.value }))}
              />
            </label>

            <label className="shared-memory-field">
              <span className="shared-memory-label">{t('sharedMemory.field.tags')}</span>
              <input
                type="text"
                className="shared-memory-input mono"
                data-testid="shared-memory-input-tags"
                value={draft.tagsText}
                onChange={(event) => setDraft((prev) => ({ ...prev, tagsText: event.target.value }))}
              />
            </label>

            <div className="shared-memory-editor-foot">
              <div className="shared-memory-toggles">
                <label className="shared-memory-check">
                  <input
                    type="checkbox"
                    checked={draft.autoInject}
                    data-testid="shared-memory-toggle-inject"
                    onChange={(event) => setDraft((prev) => ({ ...prev, autoInject: event.target.checked }))}
                  />
                  <span>{t('sharedMemory.autoInject')}</span>
                </label>

                <label className="shared-memory-check">
                  <input
                    type="checkbox"
                    checked={draft.pinned}
                    data-testid="shared-memory-toggle-pinned"
                    onChange={(event) => setDraft((prev) => ({ ...prev, pinned: event.target.checked }))}
                  />
                  <Icon name="pin" size={12} weight={2} />
                  <span>{t('sharedMemory.pinned')}</span>
                </label>
              </div>

              <div className="shared-memory-editor-actions">
                {draft.id && (
                  <button
                    type="button"
                    className="btn small danger"
                    data-testid="shared-memory-delete"
                    disabled={acting}
                    title={deleteArmed === draft.id ? t('sharedMemory.deleteConfirm') : t('savedPrompts.delete')}
                    onClick={() => {
                      // A delete reaches the other devices this memory syncs to: the first click says so, the second deletes.
                      if (!draft.id) return;
                      if (deleteArmed !== draft.id) {
                        setDeleteArmed(draft.id);
                        return;
                      }
                      setDeleteArmed(null);
                      void handleDeleteEntry(draft.id);
                    }}
                  >
                    <Icon name="trash" size={12} weight={2} />
                    <span>{deleteArmed === draft.id ? t('sharedMemory.deleteConfirm') : t('savedPrompts.delete')}</span>
                  </button>
                )}
                <button
                  type="button"
                  className="btn small primary"
                  data-testid="shared-memory-save"
                  disabled={acting || !canSaveMemoryDraft(draft)}
                  onClick={() => {
                    void handleSaveEntry();
                  }}
                >
                  <Icon name={draft.id ? 'pencil' : 'check'} size={12} weight={2} />
                  <span>{t('savedPrompts.save')}</span>
                </button>
              </div>
            </div>
          </section>
        </div>
      )}

      {tab === 'conflicts' && (
        <div className="shared-memory-conflicts" data-testid="shared-memory-conflicts-view">
          {conflicts.length === 0 ? (
            <div className="shared-memory-empty centered" data-testid="shared-memory-conflicts-empty">
              {t('sharedMemory.conflict.empty')}
            </div>
          ) : (
            conflicts.map((conflict) => (
              <article
                key={conflict.id}
                className="shared-memory-conflict-card"
                data-testid={'shared-memory-conflict-' + conflict.id}
              >
                <header className="shared-memory-conflict-head">
                  <div className="shared-memory-conflict-title">
                    <Icon name="triangle-alert" size={14} weight={2} />
                    <strong className="mono">{conflict.key}</strong>
                  </div>
                  <div className="shared-memory-conflict-actions">
                    <button
                      type="button"
                      className="btn small"
                      data-testid={'conflict-keep-local-' + conflict.id}
                      disabled={acting}
                      onClick={() => {
                        void handleResolveConflict(conflict.id, 'keepLocal');
                      }}
                    >
                      <Icon name="check" size={12} weight={2} />
                      <span>{t('sharedMemory.conflict.keepLocal')}</span>
                    </button>
                    <button
                      type="button"
                      className="btn small primary"
                      data-testid={'conflict-keep-remote-' + conflict.id}
                      disabled={acting}
                      onClick={() => {
                        void handleResolveConflict(conflict.id, 'keepRemote');
                      }}
                    >
                      <Icon name="globe" size={12} weight={2} />
                      <span>{t('sharedMemory.conflict.keepRemote')}</span>
                    </button>
                  </div>
                </header>

                <div className="shared-memory-conflict-versions">
                  <div className="shared-memory-version-box">
                    <div className="shared-memory-version-meta mono">
                      {conflict.localVersion.updatedByDeviceName} · rev {conflict.localVersion.revision}
                    </div>
                    <pre className="shared-memory-version-pre mono">{conflict.localVersion.content}</pre>
                  </div>

                  <div className="shared-memory-version-box">
                    <div className="shared-memory-version-meta mono">
                      {conflict.remoteVersion.updatedByDeviceName} · rev {conflict.remoteVersion.revision}
                    </div>
                    <pre className="shared-memory-version-pre mono">{conflict.remoteVersion.content}</pre>
                  </div>
                </div>
              </article>
            ))
          )}
        </div>
      )}

      {tab === 'settings' && (
        <div className="shared-memory-settings" data-testid="shared-memory-settings-view">
          <label className="shared-memory-setting-row">
            <input
              type="checkbox"
              checked={autoInjectGlobal}
              data-testid="shared-memory-setting-autoinject"
              onChange={(event) => setAutoInjectGlobal(event.target.checked)}
            />
            <span>{t('sharedMemory.autoInject')}</span>
          </label>

          <label className="shared-memory-setting-row">
            <input
              type="checkbox"
              checked={shareProject}
              data-testid="shared-memory-setting-share-project"
              onChange={(event) => setShareProject(event.target.checked)}
            />
            <span>{t('sharedMemory.settings.shareProject')}</span>
          </label>

          <label className="shared-memory-setting-row">
            <input
              type="checkbox"
              checked={shareUser}
              data-testid="shared-memory-setting-share-user"
              onChange={(event) => setShareUser(event.target.checked)}
            />
            <span>{t('sharedMemory.settings.shareUser')}</span>
          </label>

          <label className="shared-memory-field">
            <span className="shared-memory-label">{t('sharedMemory.settings.endpointUrl')}</span>
            <input
              type="url"
              className="shared-memory-input mono"
              data-testid="shared-memory-setting-url"
              placeholder="https://"
              value={endpointUrl}
              onChange={(event) => setEndpointUrl(event.target.value)}
            />
          </label>

          <label className="shared-memory-field">
            <span className="shared-memory-label">{t('sharedMemory.settings.endpointToken')}</span>
            <input
              type="password"
              className="shared-memory-input mono"
              data-testid="shared-memory-setting-token"
              value={endpointToken}
              onChange={(event) => setEndpointToken(event.target.value)}
            />
          </label>

          <div className="shared-memory-settings-actions">
            <button
              type="button"
              className="btn small primary"
              data-testid="shared-memory-settings-save"
              disabled={acting}
              onClick={() => {
                void handleSaveSettings();
              }}
            >
              <Icon name="check" size={12} weight={2} />
              <span>{t('sharedMemory.settings.save')}</span>
            </button>
          </div>
        </div>
      )}
    </DialogShell>
  );

  return renderInBodyPortal(dialog);
}

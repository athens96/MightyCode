// Issue & Wiki Picker panel: browse MightyWork tasks, MightyWork wiki pages, and GitHub issues,
// configure workspace integration settings & OAuth/token auth, attach formatted context to the composer,
// or launch an agent pane / git worktree directly from an item.
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import './IssuePickerPanel.css';

export interface WorkspaceIntegrationStatus {
  workspaceId: string;
  workspaceName: string;
  isGitRepo: boolean;
  mightyworkProject: string;
  detectedMightyworkProject: string;
  mightyworkUrl: string;
  mightyworkConnected: boolean;
  mightyworkProjects: Array<{ key: string; name: string }>;
  githubRepo: string;
  detectedGithubRepo: string;
  mightyworkAutoSync: boolean;
}

export interface IntegrationChecklistItem {
  text: string;
  done: boolean;
}

export interface IntegrationItem {
  source: 'mightywork_task' | 'mightywork_wiki' | 'github_issue';
  key: string;
  title: string;
  stage?: string;
  priority?: string;
  tags?: string[];
  updatedAt?: string;
  url?: string;
}

export interface IntegrationItemDetail extends IntegrationItem {
  body: string;
  checklist: IntegrationChecklistItem[];
  formattedContext: string;
}

export interface IssuePickerPanelProps {
  open: boolean;
  workspaceId: string | null;
  paneId: string | null;
  initialSettingsOpen?: boolean;
  onClose: () => void;
  onAttachToComposer: (text: string) => void;
}

export type IntegrationTab = 'tasks' | 'wiki' | 'github';
export type TaskStageFilter = 'all' | 'todo' | 'in_progress' | 'backlog';
export type GithubStateFilter = 'open' | 'all';

const TAB_KEYS: Record<IntegrationTab, string> = {
  tasks: 'integrations.tab.tasks',
  wiki: 'integrations.tab.wiki',
  github: 'integrations.tab.github',
};

const TASK_FILTER_ORDER: readonly TaskStageFilter[] = ['all', 'todo', 'in_progress', 'backlog'];
const TASK_FILTER_KEYS: Record<TaskStageFilter, string> = {
  all: 'integrations.filter.all',
  todo: 'integrations.filter.todo',
  in_progress: 'integrations.filter.inProgress',
  backlog: 'integrations.filter.backlog',
};

const GITHUB_FILTER_ORDER: readonly GithubStateFilter[] = ['open', 'all'];
const GITHUB_FILTER_KEYS: Record<GithubStateFilter, string> = {
  open: 'integrations.filter.open',
  all: 'integrations.filter.all',
};

const TAB_SOURCE: Record<IntegrationTab, IntegrationItem['source']> = {
  tasks: 'mightywork_task',
  wiki: 'mightywork_wiki',
  github: 'github_issue',
};

/** Default stage/state filter when switching tabs. */
export function defaultFilterForTab(tab: IntegrationTab): string {
  return tab === 'github' ? 'open' : 'all';
}

/** Whether the settings drawer should auto-open when status loads. */
export function shouldAutoExpandSettings(
  status: WorkspaceIntegrationStatus | null,
  initialSettingsOpen?: boolean,
): boolean {
  if (initialSettingsOpen) return true;
  if (!status) return false;
  const hasMightywork =
    status.mightyworkConnected &&
    Boolean((status.mightyworkProject || status.detectedMightyworkProject).trim());
  const hasGithub = Boolean((status.githubRepo || status.detectedGithubRepo).trim());
  return !hasMightywork && !hasGithub;
}

/** Normalizes a stage string (e.g. `"In Progress"` -> `"in_progress"`) for comparisons. */
export function normalizeStageKey(stage?: string): string {
  return (stage ?? '').trim().toLowerCase().replace(/[\s-]+/g, '_');
}

/** Pure client-side item matcher for search queries and stage/tab filters. */
export function filterIntegrationItems(
  items: readonly IntegrationItem[],
  options: { tab?: IntegrationTab; filter?: string; query?: string },
): IntegrationItem[] {
  const expectedSource = options.tab ? TAB_SOURCE[options.tab] : null;
  const activeFilter = (options.filter ?? 'all').trim().toLowerCase();
  const terms = (options.query ?? '')
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter((part) => part.length > 0);

  return items.filter((item) => {
    if (expectedSource && item.source !== expectedSource) return false;
    if (activeFilter && activeFilter !== 'all') {
      const normalizedStage = normalizeStageKey(item.stage);
      if (item.source === 'github_issue' && activeFilter === 'open') {
        if (normalizedStage && normalizedStage !== 'open') return false;
      } else if (normalizedStage !== normalizeStageKey(activeFilter)) {
        return false;
      }
    }
    if (terms.length > 0) {
      const haystack = [item.key, item.title, item.stage ?? '', ...(item.tags ?? [])]
        .join(' ')
        .toLowerCase();
      if (!terms.every((term) => haystack.includes(term))) return false;
    }
    return true;
  });
}

/** Counts completed vs total checklist items (`"2/5"`), or `null` when empty. */
export function formatChecklistProgress(checklist: readonly IntegrationChecklistItem[]): string | null {
  if (checklist.length === 0) return null;
  const done = checklist.filter((entry) => entry.done).length;
  return `${done}/${checklist.length}`;
}

/** Allows only http/https external URLs to be opened via the host bridge. */
export function safeExternalUrl(raw?: string | null): string | null {
  if (!raw) return null;
  try {
    const parsed = new URL(raw);
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed.toString() : null;
  } catch {
    return null;
  }
}

function renderInBodyPortal(node: ReactNode) {
  if (typeof document !== 'undefined' && document.body) {
    return createPortal(node, document.body);
  }
  return node;
}

export function IssuePickerPanel(props: IssuePickerPanelProps) {
  const { open, workspaceId, paneId, initialSettingsOpen, onClose, onAttachToComposer } = props;
  const { bridge, t } = useServices();
  usePageOverlay(open);

  const [tab, setTab] = useState<IntegrationTab>('tasks');
  const [filter, setFilter] = useState<string>('all');
  const [query, setQuery] = useState('');
  const [debouncedQuery, setDebouncedQuery] = useState('');
  const [refreshNonce, setRefreshNonce] = useState(0);

  const [status, setStatus] = useState<WorkspaceIntegrationStatus | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(Boolean(initialSettingsOpen));

  // Settings form state
  const [mightyworkProject, setMightyworkProject] = useState('');
  const [mightyworkUrl, setMightyworkUrl] = useState('');
  const [githubRepo, setGithubRepo] = useState('');
  const [mightyworkAutoSync, setMightyworkAutoSync] = useState(false);
  const [tokenInput, setTokenInput] = useState('');
  const [savingSettings, setSavingSettings] = useState(false);
  const [authBusy, setAuthBusy] = useState(false);

  // List and detail state
  const [items, setItems] = useState<IntegrationItem[]>([]);
  const [listLoading, setListLoading] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const [selectedItem, setSelectedItem] = useState<{ source: IntegrationItem['source']; key: string } | null>(null);
  const [detail, setDetail] = useState<IntegrationItemDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [autoSend, setAutoSend] = useState(false);
  const [acting, setActing] = useState(false);

  const openedCycleRef = useRef(false);

  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedQuery(query);
    }, query.trim().length > 0 ? 220 : 0);
    return () => clearTimeout(timer);
  }, [query]);

  const applyLoadedStatus = useCallback(
    (next: WorkspaceIntegrationStatus, firstLoadOnOpen: boolean) => {
      setStatus(next);
      setMightyworkProject(next.mightyworkProject || next.detectedMightyworkProject || '');
      setMightyworkUrl(next.mightyworkUrl || '');
      setGithubRepo(next.githubRepo || next.detectedGithubRepo || '');
      setMightyworkAutoSync(Boolean(next.mightyworkAutoSync));
      if (firstLoadOnOpen && shouldAutoExpandSettings(next, initialSettingsOpen)) {
        setSettingsOpen(true);
      }
    },
    [initialSettingsOpen],
  );

  const reloadStatus = useCallback(
    async (firstLoadOnOpen: boolean) => {
      const next = await bridge.call<WorkspaceIntegrationStatus>('integrationsStatus', { workspaceId });
      if (next) {
        applyLoadedStatus(next, firstLoadOnOpen);
      }
      return next;
    },
    [bridge, workspaceId, applyLoadedStatus],
  );

  useEffect(() => {
    if (!open) {
      openedCycleRef.current = false;
      return undefined;
    }
    let live = true;
    const isFirstOpen = !openedCycleRef.current;
    if (isFirstOpen) {
      openedCycleRef.current = true;
      setSettingsOpen(Boolean(initialSettingsOpen));
      setListError(null);
    }
    void bridge
      .call<WorkspaceIntegrationStatus>('integrationsStatus', { workspaceId })
      .then(
        (next) => {
          if (!live || !next) return;
          applyLoadedStatus(next, isFirstOpen);
        },
        (reason: unknown) => {
          if (!live) return;
          setListError(reason instanceof Error ? reason.message : String(reason));
        },
      );
    return () => {
      live = false;
    };
  }, [open, workspaceId, initialSettingsOpen, bridge, applyLoadedStatus]);

  useEffect(() => {
    if (!open) return undefined;
    let live = true;
    setListLoading(true);
    setListError(null);
    void bridge
      .call<{ items: IntegrationItem[]; error?: string }>('integrationsList', {
        workspaceId,
        tab,
        query: debouncedQuery,
        filter,
      })
      .then(
        (res) => {
          if (!live) return;
          const nextItems = res?.items ?? [];
          setItems(nextItems);
          if (res?.error) {
            setListError(res.error);
          }
          setSelectedItem((current) => {
            if (
              current &&
              nextItems.some((entry) => entry.source === current.source && entry.key === current.key)
            ) {
              return current;
            }
            const first = nextItems[0];
            return first ? { source: first.source, key: first.key } : null;
          });
          setListLoading(false);
        },
        (reason: unknown) => {
          if (!live) return;
          setItems([]);
          setSelectedItem(null);
          setListLoading(false);
          setListError(reason instanceof Error ? reason.message : String(reason));
        },
      );
    return () => {
      live = false;
    };
  }, [open, workspaceId, tab, debouncedQuery, filter, refreshNonce, bridge]);

  useEffect(() => {
    if (!open || !selectedItem) {
      setDetail(null);
      setDetailLoading(false);
      return undefined;
    }
    let live = true;
    setDetail(null);
    setDetailLoading(true);
    void bridge
      .call<{ item: IntegrationItemDetail }>('integrationsDetail', {
        workspaceId,
        source: selectedItem.source,
        key: selectedItem.key,
      })
      .then(
        (res) => {
          if (!live) return;
          setDetail(res?.item ?? null);
          setDetailLoading(false);
        },
        (reason: unknown) => {
          if (!live) return;
          setDetail(null);
          setDetailLoading(false);
          setListError(reason instanceof Error ? reason.message : String(reason));
        },
      );
    return () => {
      live = false;
    };
  }, [open, workspaceId, selectedItem, bridge]);

  const projectSuggestions = useMemo(() => {
    const map = new Map<string, string>();
    if (status?.detectedMightyworkProject) {
      map.set(status.detectedMightyworkProject, status.detectedMightyworkProject);
    }
    for (const proj of status?.mightyworkProjects ?? []) {
      map.set(proj.key, proj.name ? `${proj.key} · ${proj.name}` : proj.key);
    }
    return Array.from(map.entries()).map(([key, label]) => ({ key, label }));
  }, [status]);

  if (!open) return null;

  const switchTab = (nextTab: IntegrationTab) => {
    setTab(nextTab);
    setFilter(defaultFilterForTab(nextTab));
  };

  const triggerRefresh = () => {
    setRefreshNonce((n) => n + 1);
  };

  const handleOAuthLogin = async () => {
    if (authBusy) return;
    setAuthBusy(true);
    setListError(null);
    try {
      await bridge.call('integrationsOAuthLogin', {
        workspaceId,
        mightyworkUrl: mightyworkUrl.trim(),
      });
      await reloadStatus(false);
      triggerRefresh();
    } catch (reason: unknown) {
      setListError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setAuthBusy(false);
    }
  };

  const handleSaveToken = async () => {
    const trimmedToken = tokenInput.trim();
    if (!trimmedToken || authBusy) return;
    setAuthBusy(true);
    setListError(null);
    try {
      await bridge.call('integrationsSaveToken', {
        workspaceId,
        mightyworkUrl: mightyworkUrl.trim(),
        token: trimmedToken,
      });
      setTokenInput('');
      await reloadStatus(false);
      triggerRefresh();
    } catch (reason: unknown) {
      setListError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setAuthBusy(false);
    }
  };

  const handleLogout = async () => {
    if (authBusy) return;
    setAuthBusy(true);
    setListError(null);
    try {
      await bridge.call('integrationsLogout', {
        workspaceId,
        mightyworkUrl: mightyworkUrl.trim(),
      });
      await reloadStatus(false);
      triggerRefresh();
    } catch (reason: unknown) {
      setListError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setAuthBusy(false);
    }
  };

  const handleSaveSettings = async () => {
    if (savingSettings) return;
    setSavingSettings(true);
    setListError(null);
    try {
      await bridge.call('integrationsSaveSettings', {
        workspaceId,
        mightyworkProject: mightyworkProject.trim(),
        mightyworkUrl: mightyworkUrl.trim(),
        githubRepo: githubRepo.trim(),
        mightyworkAutoSync,
      });
      await reloadStatus(false);
      triggerRefresh();
    } catch (reason: unknown) {
      setListError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSavingSettings(false);
    }
  };

  const handleAttach = () => {
    if (!detail || paneId === null || acting) return;
    onAttachToComposer(detail.formattedContext);
    onClose();
  };

  const handleStart = async (mode: 'pane' | 'worktree') => {
    if (!detail || !workspaceId || acting) return;
    if (mode === 'worktree' && !status?.isGitRepo) return;
    setActing(true);
    setListError(null);
    try {
      await bridge.call('integrationsStart', {
        workspaceId,
        mode,
        autoSend,
        source: detail.source,
        key: detail.key,
      });
      onClose();
    } catch (reason: unknown) {
      setListError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const connected = Boolean(status?.mightyworkConnected);
  const externalUrl = safeExternalUrl(detail?.url);
  const checklistProgress = detail ? formatChecklistProgress(detail.checklist) : null;

  const dialog = (
    <DialogShell
      kind="panel"
      panel="issuePicker"
      width={840}
      height={580}
      minWidth={600}
      minHeight={420}
      label={t('integrations.title')}
      title={t('integrations.title')}
      subtitle={t('integrations.subtitle')}
      icon={<Icon name="list-todo" size={18} weight={1.8} />}
      busy={acting}
      onCancel={onClose}
      testId="issue-picker-panel"
      closeTestId="issue-picker-close"
      bodyClassName="issue-picker-body"
      backdrop={{ 'data-page-overlay': '' }}
      status={listError ? <span className="tone-text-err">{listError}</span> : undefined}
    >
      <div className="issue-picker-topbar">
        <div className="issue-picker-tabs" role="tablist" aria-label={t('integrations.title')}>
          {(['tasks', 'wiki', 'github'] as const).map((tabId) => (
            <button
              key={tabId}
              type="button"
              role="tab"
              aria-selected={tab === tabId}
              className={'issue-picker-tab' + (tab === tabId ? ' active' : '')}
              data-testid={'issue-picker-tab-' + tabId}
              onClick={() => switchTab(tabId)}
            >
              {t(TAB_KEYS[tabId])}
            </button>
          ))}
        </div>

        <div className="issue-picker-topbar-right">
          <span
            className={'issue-picker-conn-badge' + (connected ? ' connected' : '')}
            data-testid="issue-picker-conn-badge"
          >
            <span className="issue-picker-conn-dot" aria-hidden="true" />
            <span>
              {connected ? t('integrations.auth.connected') : t('integrations.auth.disconnected')}
            </span>
          </span>
          <button
            type="button"
            className={'btn small' + (settingsOpen ? ' active-toggle' : '')}
            data-testid="issue-picker-settings-toggle"
            aria-expanded={settingsOpen}
            onClick={() => setSettingsOpen((prev) => !prev)}
          >
            <Icon name="sliders-horizontal" size={12} weight={2} />
            <span>{t('integrations.settings.toggle')}</span>
          </button>
        </div>
      </div>

      {settingsOpen && (
        <section className="issue-picker-settings" data-testid="issue-picker-settings">
          <div className="issue-picker-settings-grid">
            <label className="issue-picker-field-label" htmlFor="mw-project-input">
              {t('integrations.settings.mightyworkProject')}
            </label>
            <div className="issue-picker-field-row">
              <input
                id="mw-project-input"
                type="text"
                className="dialog-field issue-picker-input mono"
                data-testid="mw-project-input"
                placeholder={t('integrations.settings.mightyworkProjectPlaceholder')}
                value={mightyworkProject}
                onChange={(event) => setMightyworkProject(event.target.value)}
              />
              {projectSuggestions.length > 0 && (
                <select
                  className="dialog-field issue-picker-select"
                  aria-label={t('integrations.settings.mightyworkProject')}
                  value={projectSuggestions.some((p) => p.key === mightyworkProject) ? mightyworkProject : ''}
                  onChange={(event) => {
                    if (event.target.value) setMightyworkProject(event.target.value);
                  }}
                >
                  <option value="">--</option>
                  {projectSuggestions.map((proj) => (
                    <option key={proj.key} value={proj.key}>
                      {proj.label}
                    </option>
                  ))}
                </select>
              )}
            </div>

            <label className="issue-picker-field-label" htmlFor="mw-url-input">
              {t('integrations.settings.mightyworkUrl')}
            </label>
            <div className="issue-picker-field-row">
              <input
                id="mw-url-input"
                type="url"
                className="dialog-field issue-picker-input-wide mono"
                data-testid="mw-url-input"
                value={mightyworkUrl}
                onChange={(event) => setMightyworkUrl(event.target.value)}
              />
            </div>

            <span className="issue-picker-field-label">MightyWork</span>
            <div className="issue-picker-field-row issue-picker-auth-row">
              {connected ? (
                <button
                  type="button"
                  className="btn"
                  data-testid="mw-logout-btn"
                  disabled={authBusy}
                  onClick={() => {
                    void handleLogout();
                  }}
                >
                  {t('integrations.auth.logout')}
                </button>
              ) : (
                <>
                  <button
                    type="button"
                    className="btn primary"
                    data-testid="mw-oauth-login-btn"
                    disabled={authBusy}
                    onClick={() => {
                      void handleOAuthLogin();
                    }}
                  >
                    {t('integrations.auth.login')}
                  </button>
                  <input
                    type="password"
                    className="dialog-field issue-picker-token-input mono"
                    data-testid="mw-token-input"
                    placeholder={t('integrations.auth.tokenPlaceholder')}
                    value={tokenInput}
                    disabled={authBusy}
                    onChange={(event) => setTokenInput(event.target.value)}
                  />
                  <button
                    type="button"
                    className="btn"
                    data-testid="mw-save-token-btn"
                    disabled={authBusy || tokenInput.trim().length === 0}
                    onClick={() => {
                      void handleSaveToken();
                    }}
                  >
                    {t('integrations.auth.saveToken')}
                  </button>
                </>
              )}
            </div>

            <label className="issue-picker-field-label" htmlFor="gh-repo-input">
              {t('integrations.settings.githubRepo')}
            </label>
            <div className="issue-picker-field-row">
              <input
                id="gh-repo-input"
                type="text"
                className="dialog-field issue-picker-input-wide mono"
                data-testid="gh-repo-input"
                placeholder={t('integrations.settings.githubRepoPlaceholder')}
                value={githubRepo}
                onChange={(event) => setGithubRepo(event.target.value)}
              />
            </div>
          </div>

          <div className="issue-picker-settings-foot">
            <label className="issue-picker-checkbox">
              <input
                type="checkbox"
                data-testid="mw-autosync-checkbox"
                checked={mightyworkAutoSync}
                onChange={(event) => setMightyworkAutoSync(event.target.checked)}
              />
              <span>{t('integrations.settings.autoSync')}</span>
            </label>
            <button
              type="button"
              className="btn primary"
              data-testid="integrations-save-settings-btn"
              disabled={savingSettings}
              onClick={() => {
                void handleSaveSettings();
              }}
            >
              {t('integrations.settings.save')}
            </button>
          </div>
        </section>
      )}

      <div className="issue-picker-columns">
        <aside className="issue-picker-list-pane">
          <div className="issue-picker-search-row">
            <div className="issue-picker-search-box">
              <Icon name="search" size={13} weight={2} />
              <input
                type="search"
                className="issue-picker-search-input"
                data-autofocus
                data-testid="issue-picker-search"
                placeholder={t('integrations.search.placeholder')}
                aria-label={t('integrations.search.placeholder')}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
              />
            </div>
            <button
              type="button"
              className="icon-button"
              data-testid="issue-picker-refresh"
              aria-label={t('integrations.refresh')}
              title={t('integrations.refresh')}
              disabled={listLoading}
              onClick={triggerRefresh}
            >
              <Icon name="refresh-cw" size={13} weight={2} />
            </button>
          </div>

          {tab === 'tasks' && (
            <div className="issue-picker-filters" role="group">
              {TASK_FILTER_ORDER.map((chip) => (
                <button
                  key={chip}
                  type="button"
                  className={'issue-picker-chip' + (filter === chip ? ' active' : '')}
                  data-testid={'issue-picker-filter-' + chip}
                  onClick={() => setFilter(chip)}
                >
                  {t(TASK_FILTER_KEYS[chip])}
                </button>
              ))}
            </div>
          )}

          {tab === 'github' && (
            <div className="issue-picker-filters" role="group">
              {GITHUB_FILTER_ORDER.map((chip) => (
                <button
                  key={chip}
                  type="button"
                  className={'issue-picker-chip' + (filter === chip ? ' active' : '')}
                  data-testid={'issue-picker-filter-' + chip}
                  onClick={() => setFilter(chip)}
                >
                  {t(GITHUB_FILTER_KEYS[chip])}
                </button>
              ))}
            </div>
          )}

          <div className="issue-picker-rows" role="listbox" aria-label={t(TAB_KEYS[tab])}>
            {listLoading ? (
              <DialogLoading rows={3} />
            ) : items.length === 0 ? (
              <div className="issue-picker-empty">–</div>
            ) : (
              items.map((item) => {
                const isSelected =
                  selectedItem?.source === item.source && selectedItem?.key === item.key;
                return (
                  <button
                    key={item.source + ':' + item.key}
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    className={'issue-picker-row' + (isSelected ? ' selected' : '')}
                    data-testid={'issue-picker-item-' + item.key}
                    onClick={() => setSelectedItem({ source: item.source, key: item.key })}
                  >
                    <div className="issue-picker-row-top">
                      <span className="issue-picker-row-key mono">{item.key}</span>
                      {item.stage && <span className="issue-picker-stage-pill">{item.stage}</span>}
                    </div>
                    <span className="issue-picker-row-title">{item.title}</span>
                  </button>
                );
              })
            )}
          </div>
        </aside>

        <section className="issue-picker-detail-pane">
          {detailLoading ? (
            <DialogLoading rows={3} />
          ) : detail ? (
            <>
              <div className="issue-picker-detail-scroll">
                <header className="issue-picker-detail-head">
                  <div className="issue-picker-detail-meta">
                    <span className="issue-picker-detail-key mono">{detail.key}</span>
                    {detail.stage && <span className="issue-picker-stage-pill">{detail.stage}</span>}
                    {detail.priority && (
                      <span className="issue-picker-tag-pill">{detail.priority}</span>
                    )}
                    {detail.tags?.map((tag) => (
                      <span key={tag} className="issue-picker-tag-pill">
                        {tag}
                      </span>
                    ))}
                    {externalUrl && (
                      <button
                        type="button"
                        className="icon-button issue-picker-ext-btn"
                        data-testid="issue-picker-open-url"
                        title={externalUrl}
                        aria-label={externalUrl}
                        onClick={() => {
                          void bridge.call('openUrl', { url: externalUrl }).catch(() => {});
                        }}
                      >
                        <Icon name="external-link" size={13} weight={2} />
                      </button>
                    )}
                  </div>
                  <h3 className="issue-picker-detail-title selectable">{detail.title}</h3>
                </header>

                {detail.checklist.length > 0 && (
                  <div className="issue-picker-checklist">
                    <div className="issue-picker-section-title">
                      <span>{t('integrations.detail.checklist')}</span>
                      {checklistProgress && (
                        <span className="mono">{checklistProgress}</span>
                      )}
                    </div>
                    <ul className="issue-picker-checklist-items">
                      {detail.checklist.map((entry, idx) => (
                        <li
                          key={idx}
                          className={'issue-picker-checklist-item' + (entry.done ? ' done' : '')}
                        >
                          <Icon
                            name={entry.done ? 'circle-check' : 'circle'}
                            size={13}
                            weight={1.9}
                          />
                          <span className="selectable">{entry.text}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                )}

                {detail.body && (
                  <div className="issue-picker-detail-body selectable">{detail.body}</div>
                )}
              </div>

              <footer className="issue-picker-detail-foot">
                <label className="issue-picker-checkbox">
                  <input
                    type="checkbox"
                    data-testid="issue-picker-autosend"
                    checked={autoSend}
                    disabled={acting}
                    onChange={(event) => setAutoSend(event.target.checked)}
                  />
                  <span>{t('integrations.action.autoSend')}</span>
                </label>

                <div className="issue-picker-detail-actions">
                  <button
                    type="button"
                    className="btn"
                    data-testid="issue-picker-attach"
                    disabled={paneId === null || acting}
                    onClick={handleAttach}
                  >
                    <Icon name="paperclip" size={12} weight={2} />
                    <span>{t('integrations.action.attach')}</span>
                  </button>
                  <button
                    type="button"
                    className="btn"
                    data-testid="issue-picker-start-pane"
                    disabled={workspaceId === null || acting}
                    onClick={() => {
                      void handleStart('pane');
                    }}
                  >
                    <Icon name="play" size={12} weight={2} />
                    <span>{t('integrations.action.startPane')}</span>
                  </button>
                  <button
                    type="button"
                    className="btn primary"
                    data-testid="issue-picker-start-worktree"
                    disabled={!status?.isGitRepo || workspaceId === null || acting}
                    onClick={() => {
                      void handleStart('worktree');
                    }}
                  >
                    <Icon name="git-branch" size={12} weight={2} />
                    <span>{t('integrations.action.startWorktree')}</span>
                  </button>
                </div>
              </footer>
            </>
          ) : (
            <div className="issue-picker-empty">–</div>
          )}
        </section>
      </div>
    </DialogShell>
  );

  return renderInBodyPortal(dialog);
}

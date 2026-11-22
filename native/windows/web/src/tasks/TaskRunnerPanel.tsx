// Task Runner & `.mighty.yaml` trust/worktree inspector panel (`MC-8`), with the `run:` actions of `MC-25` in their own section.
// Lists `.mighty.yaml`, `package.json`, `Cargo.toml`, `Makefile`, and `justfile` tasks, manages `.mighty.yaml` content-hash
// trust, streams managed background runs (polling at 1.2s only while running), and sends failures to the active agent pane.
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import {
  TASK_POLL_INTERVAL_MS,
  filterTasks,
  findTargetAgentPane,
  handoffFailureToAgent,
  hasRunningTasks,
  isTaskBlockedByTrust,
  latestRunByTaskId,
  portBlockLabel,
  runLink,
  sourceBadgeLabel,
  splitRunActions,
  tailLines,
  type PortBlock,
  type RepoConfigStatus,
  type RepoTaskItem,
  type RepoTaskRun,
  type RepoTasksListResponse,
} from './taskRunnerModel';
import './TaskRunnerPanel.css';

export interface TaskRunnerPanelProps {
  open?: boolean;
  workspaceId?: string | null;
  paneId?: string | null;
  onClose: () => void;
}

function renderInBodyPortal(node: ReactNode) {
  if (typeof document !== 'undefined' && document.body) {
    return createPortal(node, document.body);
  }
  return node;
}

function RunStatusBadge({ run, t }: { run: RepoTaskRun; t: (key: string) => string }) {
  if (run.status === 'running') {
    return (
      <span className="task-runner-status running" data-testid={'task-status-' + run.taskId}>
        <Icon name="loader-circle" size={11} weight={2.2} className="task-runner-spin" />
        <span>{t('projectTasks.running')}</span>
      </span>
    );
  }
  if (run.status === 'succeeded') {
    return (
      <span className="task-runner-status succeeded" data-testid={'task-status-' + run.taskId}>
        <Icon name="circle-check" size={11} weight={2.2} />
        <span>{t('projectTasks.succeeded')}</span>
      </span>
    );
  }
  if (run.status === 'failed') {
    return (
      <span className="task-runner-status failed" data-testid={'task-status-' + run.taskId}>
        <Icon name="circle-x" size={11} weight={2.2} />
        <span>
          {t('projectTasks.failed')}
          {typeof run.exitCode === 'number' ? ` (${run.exitCode})` : ''}
        </span>
      </span>
    );
  }
  return null;
}

export function TaskRunnerPanel({
  open = true,
  workspaceId: propWorkspaceId,
  paneId: propPaneId,
  onClose,
}: TaskRunnerPanelProps) {
  const { bridge, store, t } = useServices();
  usePageOverlay(open);

  const fallbackWorkspaceId = useStoreSelector((state) => state.snapshot?.activeWorkspaceId ?? null);
  const fallbackPaneId = useStoreSelector((state) => state.snapshot?.activeSessionId ?? null);
  const workspaceId = propWorkspaceId !== undefined ? propWorkspaceId : fallbackWorkspaceId;
  const paneId = propPaneId !== undefined ? propPaneId : fallbackPaneId;

  const [config, setConfig] = useState<RepoConfigStatus | null>(null);
  const [tasks, setTasks] = useState<RepoTaskItem[]>([]);
  const [runs, setRuns] = useState<RepoTaskRun[]>([]);
  const [folder, setFolder] = useState<string | null>(null);
  const [portBlock, setPortBlock] = useState<PortBlock | null>(null);
  const [loading, setLoading] = useState(false);
  const [acting, setActing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);

  const applyResponse = useCallback((res: RepoTasksListResponse | null | undefined) => {
    if (!res) return;
    const nextTasks = res.tasks ?? [];
    const nextRuns = res.runs ?? [];
    setConfig(res.config ?? null);
    setTasks(nextTasks);
    setRuns(nextRuns);
    setFolder(res.folder ?? null);
    setPortBlock(res.portBlock ?? null);
    setSelectedTaskId((current) => {
      if (current && nextTasks.some((item) => item.id === current)) return current;
      const runningOrFailed = nextRuns.find((r) => r.status === 'running' || r.status === 'failed');
      if (runningOrFailed && nextTasks.some((item) => item.id === runningOrFailed.taskId)) {
        return runningOrFailed.taskId;
      }
      return nextTasks[0]?.id ?? null;
    });
  }, []);

  const refresh = useCallback(
    async (showSkeleton: boolean) => {
      if (showSkeleton) setLoading(true);
      try {
        const res = await bridge.call<RepoTasksListResponse>('repoTasksList', { workspaceId, paneId });
        applyResponse(res);
        setError(null);
      } catch (reason: unknown) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        if (showSkeleton) setLoading(false);
      }
    },
    [bridge, workspaceId, paneId, applyResponse],
  );

  useEffect(() => {
    if (!open) return undefined;
    let live = true;
    setLoading(true);
    void bridge
      .call<RepoTasksListResponse>('repoTasksList', { workspaceId, paneId })
      .then(
        (res) => {
          if (!live) return;
          applyResponse(res);
          setError(null);
          setLoading(false);
        },
        (reason: unknown) => {
          if (!live) return;
          setError(reason instanceof Error ? reason.message : String(reason));
          setLoading(false);
        },
      );
    return () => {
      live = false;
    };
  }, [open, workspaceId, paneId, bridge, applyResponse]);

  const anyRunning = useMemo(() => hasRunningTasks(runs), [runs]);

  // Poll every 1.2s ONLY while open and at least one task is actively running.
  useEffect(() => {
    if (!open || !anyRunning) return undefined;
    const timer = setInterval(() => {
      void bridge
        .call<RepoTasksListResponse>('repoTasksList', { workspaceId, paneId })
        .then(applyResponse, () => {});
    }, TASK_POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [open, anyRunning, bridge, workspaceId, paneId, applyResponse]);

  const latestByTask = useMemo(() => latestRunByTaskId(runs), [runs]);
  const { actions: runActions, commands } = useMemo(() => splitRunActions(tasks), [tasks]);
  const visibleTasks = useMemo(() => filterTasks(commands, query), [commands, query]);
  const blockLabel = portBlock ? portBlockLabel(portBlock.base, portBlock.count) : '';

  const activeRun: RepoTaskRun | null = useMemo(() => {
    if (selectedRunId) {
      const byRun = runs.find((r) => r.runId === selectedRunId);
      if (byRun) return byRun;
    }
    if (selectedTaskId) {
      const latest = latestByTask.get(selectedTaskId);
      if (latest) return latest;
    }
    return runs[0] ?? null;
  }, [runs, selectedRunId, selectedTaskId, latestByTask]);

  const selectedTask: RepoTaskItem | null = useMemo(
    () => tasks.find((item) => item.id === selectedTaskId) ?? null,
    [tasks, selectedTaskId],
  );

  if (!open) return null;

  const handleApprove = async () => {
    if (!config?.contentHash || acting) return;
    setActing(true);
    setError(null);
    try {
      await bridge.call('repoConfigApprove', {
        workspaceId,
        paneId,
        contentHash: config.contentHash,
      });
      await refresh(false);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const handleRevoke = async () => {
    if (acting) return;
    setActing(true);
    setError(null);
    try {
      await bridge.call('repoConfigRevoke', { workspaceId });
      await refresh(false);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const handleRunTask = async (task: RepoTaskItem, inTerminal: boolean) => {
    if (acting || isTaskBlockedByTrust(task, config)) return;
    setActing(true);
    setError(null);
    setSelectedTaskId(task.id);
    setSelectedRunId(null);
    try {
      const res = await bridge.call<{ run?: RepoTaskRun; inTerminal?: boolean }>('repoTaskRun', {
        workspaceId,
        paneId,
        taskId: task.id,
        inTerminal,
      });
      if (inTerminal && res?.inTerminal) {
        onClose();
        return;
      }
      if (res?.run) {
        setSelectedRunId(res.run.runId);
        setRuns((prev) => [res.run!, ...prev.filter((r) => r.runId !== res.run!.runId)]);
      }
      await refresh(false);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const handleStopRun = async (runId: string) => {
    if (acting) return;
    setActing(true);
    setError(null);
    try {
      await bridge.call('repoTaskStop', { runId });
      await refresh(false);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setActing(false);
    }
  };

  const handleAskAgentFix = (run: RepoTaskRun) => {
    const snapshot = store.getState().snapshot;
    const target = findTargetAgentPane(snapshot, workspaceId, paneId);
    if (!target) return;
    handoffFailureToAgent({
      run,
      target,
      t,
      store,
      bridge,
      onClose,
    });
  };

  const canSendFailureToAgent = Boolean(
    activeRun?.status === 'failed' && findTargetAgentPane(store.getState().snapshot, workspaceId, paneId),
  );

  const worktreeRuleChips: string[] = [];
  if (config) {
    if (config.sharePaths && config.sharePaths.length > 0) {
      worktreeRuleChips.push(`share: ${config.sharePaths.join(', ')}`);
    } else if (config.shareCount > 0) {
      worktreeRuleChips.push(`share: ${config.shareCount}`);
    }
    if (config.copyRules && config.copyRules.length > 0) {
      worktreeRuleChips.push(`copy: ${config.copyRules.join(', ')}`);
    } else if (config.copyCount > 0) {
      worktreeRuleChips.push(`copy: ${config.copyCount}`);
    }
    if (config.setupCommand) {
      worktreeRuleChips.push(`setup: ${config.setupCommand}`);
    }
    if (config.teardownCommand) {
      worktreeRuleChips.push(`teardown: ${config.teardownCommand}`);
    }
    if (typeof config.ports === 'number' && config.ports !== 10) {
      worktreeRuleChips.push(`ports: ${config.ports}`);
    }
    const envLines = config.envLines ?? config.envKeys ?? [];
    if (envLines.length > 0) {
      worktreeRuleChips.push(`env: ${envLines.join(', ')}`);
    }
  }
  const sharesFolders = Boolean(config && (config.sharePaths?.length ?? 0) + config.shareCount > 0);

  const headerTools = (
    <button
      type="button"
      className="icon-button"
      data-testid="task-runner-refresh"
      aria-label={t('projectTasks.title')}
      title={t('projectTasks.title')}
      disabled={loading || acting}
      onClick={() => {
        void refresh(false);
      }}
    >
      <Icon name="refresh-cw" size={14} weight={2} />
    </button>
  );

  const dialog = (
    <DialogShell
      kind="panel"
      panel={'taskRunner' as never}
      width={860}
      height={560}
      minWidth={580}
      minHeight={380}
      label={t('projectTasks.title')}
      title={t('projectTasks.title')}
      subtitle={folder ? <span className="mono">{folder}</span> : t('projectTasks.tasks')}
      icon={<Icon name="square-terminal" size={18} weight={1.8} />}
      tools={headerTools}
      onCancel={onClose}
      testId="task-runner-panel"
      closeTestId="task-runner-close"
      bodyClassName="task-runner-body"
      backdrop={{ 'data-page-overlay': '' }}
      status={
        error ? (
          <span className="tone-text-err" data-testid="task-runner-error">
            {error}
          </span>
        ) : undefined
      }
    >
      {config?.present && (
        <section
          className={'task-runner-config-card' + (config.approved ? ' approved' : ' pending')}
          data-testid="task-runner-config-banner"
        >
          <div className="task-runner-config-main">
            <span className="task-runner-config-icon" aria-hidden="true">
              <Icon name={config.approved ? 'badge-check' : 'shield'} size={16} weight={2} />
            </span>
            <div className="task-runner-config-text">
              <div className="task-runner-config-head">
                <strong className="task-runner-config-title">{t('projectTasks.mightyConfig')}</strong>
                <span className="task-runner-file-pill mono">{config.files.join(' + ')}</span>
                {config.shortHash && <span className="task-runner-hash mono">{config.shortHash}</span>}
                <span
                  className={'task-runner-trust-pill' + (config.approved ? ' approved' : ' pending')}
                  data-testid="task-runner-trust-state"
                >
                  {config.approved ? t('projectTasks.trustApproved') : t('projectTasks.trustPending')}
                </span>
              </div>
              {config.error ? (
                <div className="task-runner-config-err">{config.error}</div>
              ) : (
                worktreeRuleChips.length > 0 && (
                  <div className="task-runner-rules-line" data-testid="task-runner-worktree-rules">
                    <span className="task-runner-rules-label">
                      {t('projectTasks.worktreeRules', {
                        share: config.shareCount,
                        copy: config.copyCount,
                      })}
                    </span>
                    {worktreeRuleChips.map((chip) => (
                      <span key={chip} className="task-runner-rule-chip mono">
                        {chip}
                      </span>
                    ))}
                    {sharesFolders && (
                      <span className="task-runner-rules-label" data-testid="task-runner-share-note">
                        {t('projectTasks.sharedWithMain')}
                      </span>
                    )}
                  </div>
                )
              )}
            </div>
          </div>

          {!config.error && config.contentHash && (
            <div className="task-runner-config-actions">
              {config.approved ? (
                <button
                  type="button"
                  className="btn small"
                  data-testid="task-runner-revoke"
                  disabled={acting}
                  onClick={() => {
                    void handleRevoke();
                  }}
                >
                  {t('projectTasks.revoke')}
                </button>
              ) : (
                <button
                  type="button"
                  className="btn small primary"
                  data-testid="task-runner-approve"
                  disabled={acting}
                  onClick={() => {
                    void handleApprove();
                  }}
                >
                  <Icon name="badge-check" size={13} weight={2} />
                  <span>{t('projectTasks.approve')}</span>
                </button>
              )}
            </div>
          )}
        </section>
      )}

      <div className="task-runner-columns">
        <aside className="task-runner-sidebar">
          {runActions.length > 0 && (
            <section className="task-runner-actions" data-testid="task-runner-actions" aria-label={t('projectTasks.runActions')}>
              <div className="task-runner-actions-head">
                <span className="task-runner-section-label">{t('projectTasks.runActions')}</span>
                {blockLabel && portBlock && (
                  <span
                    className="task-runner-port-pill mono"
                    data-testid="task-runner-port-block"
                    title={t('projectTasks.portBlockHelp', { port: portBlock.base, count: portBlock.count })}
                  >
                    {t('projectTasks.portBlock', { range: blockLabel })}
                  </span>
                )}
              </div>
              {runActions.map((task) => {
                const run = latestByTask.get(task.id);
                const isRunning = run?.status === 'running';
                const blockedByTrust = isTaskBlockedByTrust(task, config);
                const link = runLink(run);
                const isSelected = selectedTaskId === task.id && selectedRunId === null;
                return (
                  <div
                    key={task.id}
                    className={'task-runner-row task-runner-action-row' + (isSelected ? ' selected' : '')}
                    data-testid={'run-action-' + task.name}
                    onClick={() => {
                      setSelectedTaskId(task.id);
                      setSelectedRunId(null);
                    }}
                  >
                    <div className="task-runner-row-top">
                      <span className="task-runner-row-name mono">{task.name}</span>
                      {task.concurrent === false && (
                        <span
                          className="task-runner-source-pill task-runner-solo-pill"
                          data-testid={'run-action-solo-' + task.name}
                          title={t('projectTasks.notConcurrentHelp')}
                        >
                          {t('projectTasks.notConcurrent')}
                        </span>
                      )}
                    </div>
                    <div className="task-runner-row-cmd mono" title={task.command}>
                      {task.command}
                    </div>
                    {link && (
                      <div className="task-runner-url-line" onClick={(event) => event.stopPropagation()}>
                        <button
                          type="button"
                          className="task-runner-url mono"
                          data-testid={'run-action-url-' + task.name}
                          title={link}
                          onClick={() => {
                            void bridge.call('openUrl', { url: link }).catch(() => {});
                          }}
                        >
                          {link}
                        </button>
                        {run?.urlOutsideBlock && (
                          <span
                            className="task-runner-url-note outside"
                            title={t('projectTasks.urlOutsideHelp', { range: blockLabel })}
                          >
                            {t('projectTasks.urlOutside')}
                          </span>
                        )}
                        {run?.urlListening === false && (
                          <span className="task-runner-url-note" title={t('projectTasks.urlNotListeningHelp')}>
                            {t('projectTasks.urlNotListening')}
                          </span>
                        )}
                      </div>
                    )}
                    <div className="task-runner-row-foot" onClick={(event) => event.stopPropagation()}>
                      {run ? <RunStatusBadge run={run} t={t} /> : <span />}
                      <div className="task-runner-row-btns">
                        {isRunning && run ? (
                          <button
                            type="button"
                            className="btn small danger"
                            data-testid={'run-action-stop-' + task.name}
                            disabled={acting}
                            onClick={() => {
                              void handleStopRun(run.runId);
                            }}
                          >
                            <Icon name="circle-stop" size={12} weight={2} />
                            <span>{t('projectTasks.stop')}</span>
                          </button>
                        ) : (
                          <button
                            type="button"
                            className="btn small primary"
                            data-testid={'run-action-start-' + task.name}
                            disabled={acting || blockedByTrust}
                            title={blockedByTrust ? t('projectTasks.trustPending') : t('projectTasks.start')}
                            onClick={() => {
                              void handleRunTask(task, false);
                            }}
                          >
                            <Icon name="play" size={11} weight={2.2} />
                            <span>{t('projectTasks.start')}</span>
                          </button>
                        )}
                      </div>
                    </div>
                  </div>
                );
              })}
            </section>
          )}
          <div className="task-runner-sidebar-head">
            <span className="task-runner-section-label">{t('projectTasks.tasks')}</span>
            {commands.length > 4 && (
              <div className="task-runner-search">
                <Icon name="search" size={12} weight={2} />
                <input
                  type="search"
                  className="task-runner-search-input"
                  data-testid="task-runner-filter"
                  value={query}
                  aria-label={t('projectTasks.tasks')}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </div>
            )}
          </div>

          <div className="task-runner-list" role="listbox" aria-label={t('projectTasks.tasks')}>
            {loading && commands.length === 0 ? (
              <DialogLoading rows={3} />
            ) : visibleTasks.length === 0 ? (
              <div className="task-runner-empty" data-testid="task-runner-empty">
                {t('projectTasks.noTasks')}
              </div>
            ) : (
              visibleTasks.map((task) => {
                const run = latestByTask.get(task.id);
                const isSelected = selectedTaskId === task.id && selectedRunId === null;
                const isRunning = run?.status === 'running';
                const blockedByTrust = isTaskBlockedByTrust(task, config);
                return (
                  <div
                    key={task.id}
                    role="option"
                    aria-selected={isSelected}
                    tabIndex={0}
                    className={'task-runner-row' + (isSelected ? ' selected' : '')}
                    data-testid={'task-row-' + task.id}
                    onClick={() => {
                      setSelectedTaskId(task.id);
                      setSelectedRunId(null);
                    }}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        setSelectedTaskId(task.id);
                        setSelectedRunId(null);
                      }
                    }}
                  >
                    <div className="task-runner-row-top">
                      <span className="task-runner-row-name mono">{task.name}</span>
                      <span className="task-runner-source-pill">{sourceBadgeLabel(task.source)}</span>
                    </div>
                    <div className="task-runner-row-cmd mono" title={task.command}>
                      {task.command}
                    </div>
                    {task.description && task.description !== task.command && (
                      <div className="task-runner-row-desc">{task.description}</div>
                    )}
                    <div className="task-runner-row-foot" onClick={(event) => event.stopPropagation()}>
                      {run ? <RunStatusBadge run={run} t={t} /> : <span />}
                      <div className="task-runner-row-btns">
                        {isRunning && run ? (
                          <button
                            type="button"
                            className="btn small danger"
                            data-testid={'task-stop-' + task.id}
                            disabled={acting}
                            title={t('projectTasks.stop')}
                            onClick={() => {
                              void handleStopRun(run.runId);
                            }}
                          >
                            <Icon name="circle-stop" size={12} weight={2} />
                            <span>{t('projectTasks.stop')}</span>
                          </button>
                        ) : (
                          <>
                            <button
                              type="button"
                              className="btn small"
                              data-testid={'task-terminal-' + task.id}
                              disabled={acting || blockedByTrust}
                              title={blockedByTrust ? t('projectTasks.trustPending') : t('projectTasks.runTerminal')}
                              onClick={() => {
                                void handleRunTask(task, true);
                              }}
                            >
                              <Icon name="terminal" size={12} weight={2} />
                              <span>{t('projectTasks.runTerminal')}</span>
                            </button>
                            <button
                              type="button"
                              className="btn small primary"
                              data-testid={'task-run-' + task.id}
                              disabled={acting || blockedByTrust}
                              title={blockedByTrust ? t('projectTasks.trustPending') : t('projectTasks.runNow')}
                              onClick={() => {
                                void handleRunTask(task, false);
                              }}
                            >
                              <Icon name="play" size={11} weight={2.2} />
                              <span>{t('projectTasks.runNow')}</span>
                            </button>
                          </>
                        )}
                      </div>
                    </div>
                  </div>
                );
              })
            )}
          </div>
        </aside>

        <section className="task-runner-console-pane">
          <header className="task-runner-console-head">
            <div className="task-runner-console-meta">
              <Icon name="zap" size={14} weight={2} />
              <strong className="task-runner-console-title mono">
                {activeRun?.name ?? selectedTask?.name ?? t('projectTasks.open')}
              </strong>
              {(activeRun?.command ?? selectedTask?.command) && (
                <span className="task-runner-console-cmd mono">
                  {activeRun?.command ?? selectedTask?.command}
                </span>
              )}
            </div>

            <div className="task-runner-console-actions">
              {activeRun && <RunStatusBadge run={activeRun} t={t} />}
              {activeRun?.status === 'running' && (
                <button
                  type="button"
                  className="btn small danger"
                  data-testid="task-console-stop"
                  disabled={acting}
                  onClick={() => {
                    void handleStopRun(activeRun.runId);
                  }}
                >
                  <Icon name="circle-stop" size={12} weight={2} />
                  <span>{t('projectTasks.stop')}</span>
                </button>
              )}
              {activeRun?.status === 'failed' && (
                <button
                  type="button"
                  className="btn small primary task-runner-fix-btn"
                  data-testid="task-ask-agent-fix"
                  disabled={!canSendFailureToAgent}
                  onClick={() => handleAskAgentFix(activeRun)}
                >
                  <Icon name="message-square-share" size={13} weight={2} />
                  <span>{t('projectTasks.askAgentFix')}</span>
                </button>
              )}
            </div>
          </header>

          {activeRun && tailLines(activeRun.output).length > 0 ? (
            <pre className="task-runner-output mono selectable" data-testid="task-runner-output">
              {tailLines(activeRun.output)}
            </pre>
          ) : (
            <div className="task-runner-output-empty" data-testid="task-runner-output-empty">
              {t('projectTasks.outputEmpty')}
            </div>
          )}
        </section>
      </div>
    </DialogShell>
  );

  return renderInBodyPortal(dialog);
}

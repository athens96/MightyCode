// Git Workbench panel (MC-5): branch switching/creation, file & hunk staging/unstaging,
// interactive unified diff viewer, pre-commit secret guard, one-shot AI Conventional Commit
// drafting, multi-file or staged commit, push, PR/MR lifecycle, and 1-click CI failure fix loop.
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { appendingBlock } from '../mighty/runChangesModel';
import { pickTargetAgentSession } from '../panes/files/editorModel';
import { usePageOverlay } from '../shell/overlay';
import { ChecksTab } from './ChecksTab';
import {
  checkBadgeMeta,
  countStagedFiles,
  defaultDiffStagedTab,
  fileMarkTone,
  formatAgentCommitPrompt,
  formatCiFixPrompt,
  formatSecretFindings,
  isValidBranchName,
  prStateLocaleKey,
  reconcileSelectedPaths,
  safeExternalHttpUrl,
  toggleAllSelectablePaths,
  type GitCommitResult,
  type GitCreatePrResult,
  type GitDraftMessageResult,
  type GitFailedCheckLogResult,
  type GitPrStatusResult,
  type GitWorkbenchDiff,
  type GitWorkbenchFile,
  type GitWorkbenchStatus,
  type SecretFinding,
} from './gitWorkbenchModel';
import './GitWorkbenchPanel.css';

export type GitWorkbenchTab = 'changes' | 'checks';

export interface GitWorkbenchPanelProps {
  open: boolean;
  workspaceId?: string | null;
  paneId?: string | null;
  /** The tab it opens on (the review queue's checks summary opens Checks). */
  initialTab?: GitWorkbenchTab;
  onClose: () => void;
}

function renderInBodyPortal(node: ReactNode) {
  if (typeof document !== 'undefined' && document.body) {
    return createPortal(node, document.body);
  }
  return node;
}

export function GitWorkbenchPanel(props: GitWorkbenchPanelProps) {
  const { open, workspaceId: propWorkspaceId, paneId: propPaneId, initialTab, onClose } = props;
  const { bridge, store, t } = useServices();
  usePageOverlay(open);

  const activeWorkspaceId = useStoreSelector((state) => state.snapshot?.activeWorkspaceId ?? null);
  const resolvedWorkspaceId = propWorkspaceId ?? activeWorkspaceId;
  // A worktree pane's /git works on its own worktree: the host resolves the folder from the pane.
  const resolvedPaneId = propPaneId ?? null;

  const [status, setStatus] = useState<GitWorkbenchStatus | null>(null);
  const [statusLoading, setStatusLoading] = useState(false);
  const [prStatus, setPrStatus] = useState<GitPrStatusResult | null>(null);
  const [prLoading, setPrLoading] = useState(false);

  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(() => new Set());
  const knownPathsRef = useRef<Set<string>>(new Set());

  const [activeFilePath, setActiveFilePath] = useState<string | null>(null);
  const [diffStagedPreference, setDiffStagedPreference] = useState(false);
  const [diff, setDiff] = useState<GitWorkbenchDiff | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);

  const [newBranchOpen, setNewBranchOpen] = useState(false);
  const [newBranchName, setNewBranchName] = useState('');

  const [commitMessage, setCommitMessage] = useState('');
  const [aiDrafting, setAiDrafting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const [secretFindings, setSecretFindings] = useState<SecretFinding[]>([]);
  const [pendingCommitMode, setPendingCommitMode] = useState<{ stagedOnly: boolean; paths: string[] } | null>(null);
  const [diffRefreshNonce, setDiffRefreshNonce] = useState(0);
  const [tab, setTab] = useState<GitWorkbenchTab>(initialTab ?? 'changes');
  const [checksNonce, setChecksNonce] = useState(0);

  const loadAll = useCallback(
    async (silent = false) => {
      if (!resolvedWorkspaceId) return;
      if (!silent) {
        setStatusLoading(true);
        setPrLoading(true);
      }
      try {
        const nextStatus = await bridge.call<GitWorkbenchStatus>('gitWorkbenchStatus', {
          workspaceId: resolvedWorkspaceId,
          paneId: resolvedPaneId,
        });
        if (nextStatus) {
          setStatus(nextStatus);
          setSelectedPaths((prev) => {
            const reconciled = reconcileSelectedPaths(prev, knownPathsRef.current, nextStatus.files);
            knownPathsRef.current = new Set(nextStatus.files.map((f) => f.path));
            return reconciled;
          });
          setActiveFilePath((prev) => {
            if (prev && nextStatus.files.some((f) => f.path === prev)) return prev;
            return nextStatus.files[0]?.path ?? null;
          });
          setDiffRefreshNonce((n) => n + 1);
        }
      } catch (reason: unknown) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        setStatusLoading(false);
      }

      try {
        const nextPr = await bridge.call<GitPrStatusResult>('gitWorkbenchPrStatus', {
          workspaceId: resolvedWorkspaceId,
          paneId: resolvedPaneId,
        });
        if (nextPr) {
          setPrStatus(nextPr);
        }
      } catch {
        setPrStatus((prev) => prev ?? { hosting: 'none', pr: null });
      } finally {
        setPrLoading(false);
      }
    },
    [bridge, resolvedWorkspaceId, resolvedPaneId],
  );

  useEffect(() => {
    if (!open) {
      knownPathsRef.current = new Set();
      setSecretFindings([]);
      setPendingCommitMode(null);
      setNewBranchOpen(false);
      setNewBranchName('');
      return undefined;
    }
    setError(null);
    setNote(null);
    // The Checks tab reads its own model; the changes are read when their tab shows.
    if (tab === 'changes') void loadAll(false);
    return undefined;
  }, [open, tab, resolvedWorkspaceId, resolvedPaneId, loadAll]);

  const activeFile: GitWorkbenchFile | null = useMemo(() => {
    if (!status || !activeFilePath) return null;
    return status.files.find((f) => f.path === activeFilePath) ?? null;
  }, [status, activeFilePath]);

  const viewingStagedDiff = useMemo(
    () => defaultDiffStagedTab(activeFile, diffStagedPreference),
    [activeFile, diffStagedPreference],
  );

  useEffect(() => {
    if (!open || !resolvedWorkspaceId || !activeFile) {
      setDiff(null);
      setDiffLoading(false);
      return undefined;
    }
    let live = true;
    setDiffLoading(true);
    void bridge
      .call<GitWorkbenchDiff>('gitWorkbenchDiff', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        path: activeFile.path,
        staged: viewingStagedDiff,
      })
      .then(
        (res) => {
          if (!live) return;
          setDiff(res ?? null);
          setDiffLoading(false);
        },
        (reason: unknown) => {
          if (!live) return;
          setDiff(null);
          setDiffLoading(false);
          setError(reason instanceof Error ? reason.message : String(reason));
        },
      );
    return () => {
      live = false;
    };
  }, [open, resolvedWorkspaceId, resolvedPaneId, activeFile, viewingStagedDiff, diffRefreshNonce, bridge]);

  const injectPromptIntoTargetAgent = useCallback(
    (promptText: string): boolean => {
      const snap = store.getState().snapshot;
      const target = pickTargetAgentSession(
        snap?.sessions ?? [],
        resolvedWorkspaceId ?? undefined,
        snap?.activeSessionId,
        propPaneId,
      );
      if (!target) {
        setError(t('git.agent.noPane'));
        return false;
      }
      const merged = appendingBlock(promptText, target.draft ?? '');
      const scheduleAfterClose =
        typeof requestAnimationFrame === 'function' ? requestAnimationFrame : queueMicrotask;
      onClose();
      scheduleAfterClose(() => {
        void bridge.call('updateDraft', { paneId: target.id, text: merged }).catch(() => {});
        store.dispatch({ kind: 'fill', paneId: target.id, text: promptText, append: true });
        store.dispatch({ kind: 'choose', paneId: target.id });
        void bridge.call('activatePane', { id: target.id }).catch(() => {});
      });
      return true;
    },
    [store, resolvedWorkspaceId, propPaneId, t, onClose, bridge],
  );

  if (!open) return null;

  const files = status?.files ?? [];
  const stagedCount = countStagedFiles(files);
  const orderedSelectedPaths = files.filter((f) => selectedPaths.has(f.path)).map((f) => f.path);
  const selectableCount = files.filter((f) => !f.conflicted).length;
  const allSelected = selectableCount > 0 && orderedSelectedPaths.length === selectableCount;

  const runAction = async (work: () => Promise<string | null>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const doneMessage = await work();
      if (doneMessage) setNote(doneMessage);
      await loadAll(true);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleSwitchBranch = (nextBranch: string) => {
    if (!nextBranch || nextBranch === status?.head || busy) return;
    void runAction(async () => {
      await bridge.call('gitWorkbenchSwitchBranch', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        branch: nextBranch,
        create: false,
      });
      return t('git.workbench.branch', { branch: nextBranch });
    });
  };

  const handleCreateBranch = () => {
    const trimmed = newBranchName.trim();
    if (!isValidBranchName(trimmed) || busy) return;
    void runAction(async () => {
      await bridge.call('gitWorkbenchSwitchBranch', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        branch: trimmed,
        create: true,
      });
      setNewBranchName('');
      setNewBranchOpen(false);
      return t('git.workbench.branch', { branch: trimmed });
    });
  };

  const handleStageFile = (file: GitWorkbenchFile) => {
    void runAction(async () => {
      await bridge.call('gitWorkbenchStage', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        paths: [file.path],
      });
      return null;
    });
  };

  const handleUnstageFile = (file: GitWorkbenchFile) => {
    void runAction(async () => {
      await bridge.call('gitWorkbenchUnstage', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        paths: [file.path],
      });
      return null;
    });
  };

  const handleStageOrUnstageHunk = (rawPatch: string, unstage: boolean) => {
    void runAction(async () => {
      await bridge.call(unstage ? 'gitWorkbenchUnstage' : 'gitWorkbenchStage', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        patch: rawPatch,
      });
      return null;
    });
  };

  const handleDraftMessage = async () => {
    if (aiDrafting || busy) return;
    const useStaged = stagedCount > 0 && orderedSelectedPaths.length === 0;
    const targetPaths = useStaged ? undefined : orderedSelectedPaths;
    if (!useStaged && (!targetPaths || targetPaths.length === 0)) {
      setError(t('git.commit.error.empty'));
      return;
    }
    setAiDrafting(true);
    setError(null);
    try {
      const res = await bridge.call<GitDraftMessageResult>('gitWorkbenchDraftMessage', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        paths: targetPaths,
        stagedOnly: useStaged,
      });
      if (res?.message) {
        setCommitMessage(res.message);
      }
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setAiDrafting(false);
    }
  };

  const executeCommit = async (stagedOnly: boolean, paths: string[], allowSecrets: boolean) => {
    const trimmedMessage = commitMessage.trim();
    if (!trimmedMessage || (!stagedOnly && paths.length === 0)) {
      setError(t('git.commit.error.empty'));
      return;
    }
    if (busy) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const res = await bridge.call<GitCommitResult>('gitWorkbenchCommit', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        message: trimmedMessage,
        paths: stagedOnly ? undefined : paths,
        stagedOnly,
        allowSecrets,
      });
      if (res?.blockedBySecrets && res.findings.length > 0) {
        setSecretFindings(res.findings);
        setPendingCommitMode({ stagedOnly, paths });
        return;
      }
      setSecretFindings([]);
      setPendingCommitMode(null);
      setCommitMessage('');
      const committedCount = stagedOnly ? stagedCount : paths.length;
      setNote(t('git.commit.done', { count: committedCount, sha: res?.sha ?? '' }));
      await loadAll(true);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handlePush = () => {
    void runAction(async () => {
      await bridge.call('gitWorkbenchPush', { workspaceId: resolvedWorkspaceId, paneId: resolvedPaneId });
      return t('git.push.done');
    });
  };

  const handleCreatePr = () => {
    void runAction(async () => {
      const res = await bridge.call<GitCreatePrResult>('gitWorkbenchCreatePr', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
      });
      return t('git.pr.created', { url: res?.url ?? '' });
    });
  };

  const handleAskAgentToCommit = () => {
    if (orderedSelectedPaths.length === 0 || busy) return;
    const prompt = formatAgentCommitPrompt(t('git.commit.agentRequest'), orderedSelectedPaths);
    injectPromptIntoTargetAgent(prompt);
  };

  const handleFixFailedChecks = async (urlOverride?: string) => {
    const prUrl = urlOverride ?? prStatus?.pr?.url;
    if (busy || !prUrl) return;
    setBusy(true);
    setError(null);
    try {
      const logRes = await bridge.call<GitFailedCheckLogResult>('gitWorkbenchFailedCheckLog', {
        workspaceId: resolvedWorkspaceId,
        paneId: resolvedPaneId,
        branch: status?.head && !status.detached ? status.head : undefined,
      });
      const prompt = formatCiFixPrompt(t('git.pr.fixRequest', { url: prUrl }), logRes?.log);
      setBusy(false);
      injectPromptIntoTargetAgent(prompt);
    } catch (reason: unknown) {
      setBusy(false);
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };

  const hosting = prStatus?.hosting ?? status?.hosting ?? 'none';
  const activePr = prStatus?.pr ?? null;
  const prUrl = safeExternalHttpUrl(activePr?.url);
  const checkBadge = checkBadgeMeta(activePr?.checks);
  const stateKey = activePr ? prStateLocaleKey(activePr.state) : null;
  const stateLabel = activePr ? (stateKey ? t(stateKey) : activePr.state) : '';

  const subtitleText = status?.head
    ? t('git.workbench.branch', { branch: status.head }) +
      (status.ahead > 0 ? ` ↑${status.ahead}` : '') +
      (status.behind > 0 ? ` ↓${status.behind}` : '')
    : undefined;

  const statusNode = busy ? (
    <span className="git-wb-footer-busy">
      <span className="dialog-spinner" aria-hidden="true" />
      <span>{t('dialog.loading')}</span>
    </span>
  ) : error ? (
    <span className="tone-text-err selectable">{error}</span>
  ) : note ? (
    <span className="tone-text-done selectable">{note}</span>
  ) : undefined;

  const dialog = (
    <DialogShell
      kind="panel"
      panel={'gitWorkbench' as never}
      width={940}
      height={650}
      minWidth={680}
      minHeight={460}
      label={t('git.workbench.title')}
      title={t('git.workbench.title')}
      subtitle={subtitleText}
      icon={<Icon name="git-branch" size={18} weight={1.8} />}
      busy={busy}
      onCancel={onClose}
      testId="git-workbench-panel"
      closeTestId="git-workbench-close"
      bodyClassName="git-wb-body"
      backdrop={{ 'data-page-overlay': '' }}
      tools={
        <button
          type="button"
          className="icon-button"
          data-testid="git-workbench-refresh"
          aria-label={t('git.workbench.refresh')}
          title={t('git.workbench.refresh')}
          disabled={busy || statusLoading}
          onClick={() => {
            if (tab === 'checks') setChecksNonce((n) => n + 1);
            else void loadAll(false);
          }}
        >
          <Icon name="refresh-cw" size={14} weight={2} />
        </button>
      }
      status={statusNode}
      buttons={tab === 'checks' ? undefined : (
        <>
          <button
            type="button"
            className="btn"
            data-testid="git-ask-agent-btn"
            title={t('git.commit.askAgentHelp')}
            disabled={orderedSelectedPaths.length === 0 || busy}
            onClick={handleAskAgentToCommit}
          >
            <Icon name="sparkles" size={13} weight={2} />
            <span>{t('git.commit.askAgent')}</span>
          </button>
          <button
            type="button"
            className="btn"
            data-testid="git-push-btn"
            disabled={busy || Boolean(status?.detached)}
            onClick={handlePush}
          >
            <Icon name="arrow-up" size={13} weight={2} />
            <span>{t('git.push')}</span>
          </button>
          {stagedCount > 0 && (
            <button
              type="button"
              className="btn"
              data-testid="git-commit-staged-btn"
              disabled={busy || commitMessage.trim().length === 0}
              onClick={() => {
                void executeCommit(true, [], false);
              }}
            >
              <Icon name="check" size={13} weight={2} />
              <span>{t('git.commit.commitStaged', { count: stagedCount })}</span>
            </button>
          )}
          <button
            type="button"
            className="btn primary"
            data-testid="git-commit-btn"
            disabled={busy || orderedSelectedPaths.length === 0 || commitMessage.trim().length === 0}
            onClick={() => {
              void executeCommit(false, orderedSelectedPaths, false);
            }}
          >
            <span>{t('git.commit.button', { count: orderedSelectedPaths.length })}</span>
          </button>
        </>
      )}
    >
      <div className="git-wb-tabs" role="tablist" aria-label={t('git.workbench.title')}>
        <button type="button" role="tab" aria-selected={tab === 'changes'} className={'git-wb-tab' + (tab === 'changes' ? ' active' : '')}
          data-testid="git-tab-changes" onClick={() => setTab('changes')}>
          <Icon name="file-code" size={13} weight={2} />
          <span>{t('git.checks.changesTab')}</span>
        </button>
        <button type="button" role="tab" aria-selected={tab === 'checks'} className={'git-wb-tab' + (tab === 'checks' ? ' active' : '')}
          data-testid="git-tab-checks" onClick={() => setTab('checks')}>
          <Icon name="list-checks" size={13} weight={2} />
          <span>{t('git.checks.tab')}</span>
        </button>
      </div>

      {tab === 'checks' ? (
        <ChecksTab
          workspaceId={resolvedWorkspaceId ?? null}
          paneId={resolvedPaneId}
          active={open}
          refreshNonce={checksNonce}
          busy={busy}
          onFixChecks={(url) => { void handleFixFailedChecks(url); }}
          onToComposer={(text) => { if (text) injectPromptIntoTargetAgent(text); }}
        />
      ) : (<>
      {/* Branch bar + PR/CI strip */}
      <div className="git-wb-topstrip">
        <div className="git-wb-branch-bar">
          <Icon name="git-branch" size={14} weight={2} />
          <select
            className="dialog-field git-wb-branch-select mono"
            data-testid="git-branch-select"
            aria-label={t('git.workbench.switchBranch')}
            value={status?.head ?? ''}
            disabled={busy || !status}
            onChange={(event) => handleSwitchBranch(event.target.value)}
          >
            {status?.detached && <option value="(detached)">(detached)</option>}
            {(status?.branches ?? []).map((branch) => (
              <option key={branch} value={branch}>
                {branch}
              </option>
            ))}
          </select>

          {(status?.ahead ?? 0) > 0 && (
            <span className="git-wb-sync-pill" data-testid="git-ahead-pill">
              <Icon name="arrow-up" size={11} weight={2.2} />
              <span>{status?.ahead}</span>
            </span>
          )}
          {(status?.behind ?? 0) > 0 && (
            <span className="git-wb-sync-pill" data-testid="git-behind-pill">
              <Icon name="arrow-down" size={11} weight={2.2} />
              <span>{status?.behind}</span>
            </span>
          )}

          <button
            type="button"
            className={'btn git-wb-compact-btn' + (newBranchOpen ? ' active-toggle' : '')}
            data-testid="git-new-branch-toggle"
            disabled={busy}
            onClick={() => setNewBranchOpen((prev) => !prev)}
          >
            <Icon name="plus" size={12} weight={2} />
            <span>{t('git.workbench.newBranch')}</span>
          </button>

          {newBranchOpen && (
            <div className="git-wb-new-branch-inline" data-testid="git-new-branch-form">
              <input
                type="text"
                className="dialog-field git-wb-new-branch-input mono"
                data-testid="git-new-branch-input"
                placeholder="feat/..."
                aria-label={t('git.workbench.newBranch')}
                value={newBranchName}
                disabled={busy}
                onChange={(event) => setNewBranchName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    handleCreateBranch();
                  }
                }}
              />
              <button
                type="button"
                className="btn primary git-wb-compact-btn"
                data-testid="git-create-branch-btn"
                disabled={busy || !isValidBranchName(newBranchName)}
                onClick={handleCreateBranch}
              >
                {t('git.workbench.createBranch')}
              </button>
            </div>
          )}
        </div>

        <div className="git-wb-pr-bar" data-testid="git-pr-bar">
          <Icon name="git-merge" size={14} weight={2} />
          {prLoading && !prStatus ? (
            <span className="git-wb-pr-muted">{t('dialog.loading')}</span>
          ) : activePr ? (
            <>
              <span className="git-wb-pr-label">
                {t('git.pr.line', { number: activePr.number, state: stateLabel })}
              </span>
              {activePr.title && (
                <span className="git-wb-pr-title" title={activePr.title}>
                  {activePr.title}
                </span>
              )}
              {checkBadge && (
                <span
                  className={`git-wb-check-pill tone-${checkBadge.tone}`}
                  data-testid="git-pr-checks-badge"
                >
                  <Icon name={checkBadge.icon} size={12} weight={2} />
                  <span>{t(checkBadge.labelKey)}</span>
                </span>
              )}
              <span className="spacer" />
              {activePr.checks === 'failed' && (
                <button
                  type="button"
                  className="btn git-wb-compact-btn git-wb-fix-btn"
                  data-testid="git-pr-fix-checks"
                  disabled={busy}
                  onClick={() => {
                    void handleFixFailedChecks(undefined);
                  }}
                >
                  <Icon name="wrench" size={12} weight={2} />
                  <span>{t('git.pr.fixChecks')}</span>
                </button>
              )}
              {prUrl && (
                <button
                  type="button"
                  className="btn git-wb-compact-btn"
                  data-testid="git-pr-open-btn"
                  onClick={() => {
                    void bridge.call('openUrl', { url: prUrl }).catch(() => {});
                  }}
                >
                  <Icon name="external-link" size={12} weight={2} />
                  <span>{t('git.pr.open')}</span>
                </button>
              )}
            </>
          ) : hosting === 'none' ? (
            <span className="git-wb-pr-muted">{t('git.pr.noRemote')}</span>
          ) : prStatus?.cliMissing ? (
            <span className="git-wb-pr-muted">
              {t(prStatus.cliMissing === 'glab' ? 'git.pr.error.glabMissing' : 'git.pr.error.ghMissing')}
            </span>
          ) : (
            <>
              <span className="git-wb-pr-muted">{t('git.pr.none')}</span>
              <span className="spacer" />
              <button
                type="button"
                className="btn git-wb-compact-btn"
                data-testid="git-pr-create-btn"
                title={status?.onDefaultBranch ? t('git.pr.onDefaultBranch') : t('git.pr.createHelp')}
                disabled={busy || Boolean(status?.detached) || Boolean(status?.onDefaultBranch)}
                onClick={handleCreatePr}
              >
                <Icon name="git-merge" size={12} weight={2} />
                <span>{t(hosting === 'gitlab' ? 'git.pr.createGitLab' : 'git.pr.create')}</span>
              </button>
            </>
          )}
        </div>
      </div>

      {/* Pre-commit secret warning banner */}
      {secretFindings.length > 0 && (
        <section className="git-wb-secret-alert" role="alert" data-testid="git-secrets-alert">
          <div className="git-wb-secret-head">
            <Icon name="shield" size={15} weight={2} />
            <strong>{t('git.secrets.title', { count: secretFindings.length })}</strong>
          </div>
          <ul className="git-wb-secret-list mono">
            {formatSecretFindings(secretFindings).map((line, index) => (
              <li key={index}>{line}</li>
            ))}
          </ul>
          <p className="git-wb-secret-msg">{t('git.secrets.message')}</p>
          <div className="git-wb-secret-actions">
            <button
              type="button"
              className="btn"
              data-testid="git-secrets-cancel"
              disabled={busy}
              onClick={() => {
                setSecretFindings([]);
                setPendingCommitMode(null);
              }}
            >
              <Icon name="x" size={12} weight={2} />
              <span>{t('dialog.close')}</span>
            </button>
            <button
              type="button"
              className="btn danger"
              data-testid="git-secrets-commit-anyway"
              disabled={busy || !pendingCommitMode}
              onClick={() => {
                if (pendingCommitMode) {
                  void executeCommit(pendingCommitMode.stagedOnly, pendingCommitMode.paths, true);
                }
              }}
            >
              <Icon name="triangle-alert" size={12} weight={2} />
              <span>{t('git.secrets.commitAnyway')}</span>
            </button>
          </div>
        </section>
      )}

      {/* Main workbench split: Changed files list + Diff & Commit message */}
      <div className="git-wb-columns">
        <aside className="git-wb-files-pane">
          <div className="git-wb-files-head">
            <span className="git-wb-section-label">
              {t('git.commit.files', { count: files.length })}
            </span>
            {stagedCount > 0 && (
              <span className="git-wb-staged-pill" data-testid="git-staged-count">
                {t('git.workbench.stagedBadge')} · {stagedCount}
              </span>
            )}
            <span className="spacer" />
            {selectableCount > 0 && (
              <label className="git-wb-select-all">
                <input
                  type="checkbox"
                  data-testid="git-select-all-checkbox"
                  checked={allSelected}
                  disabled={busy}
                  onChange={() =>
                    setSelectedPaths((prev) => toggleAllSelectablePaths(files, prev))
                  }
                />
                <span>{t('git.workbench.selectAll')}</span>
              </label>
            )}
          </div>

          <div className="git-wb-files-list" role="listbox" aria-label={t('git.workbench.title')}>
            {statusLoading && !status ? (
              <DialogLoading rows={4} />
            ) : files.length === 0 ? (
              <div className="git-wb-empty">{t('git.commit.clean')}</div>
            ) : (
              files.map((file) => {
                const isCurrent = activeFile?.path === file.path;
                const isChecked = selectedPaths.has(file.path);
                const markTone = fileMarkTone(file.mark);
                return (
                  <div
                    key={file.path}
                    role="option"
                    aria-selected={isCurrent}
                    className={'git-wb-file-row' + (isCurrent ? ' active' : '')}
                    data-testid={'git-file-row-' + file.path}
                    onClick={() => setActiveFilePath(file.path)}
                  >
                    <input
                      type="checkbox"
                      className="git-wb-file-check"
                      aria-label={file.path}
                      checked={isChecked}
                      disabled={busy || file.conflicted}
                      onClick={(event) => event.stopPropagation()}
                      onChange={(event) => {
                        const checked = event.target.checked;
                        setSelectedPaths((prev) => {
                          const next = new Set(prev);
                          if (checked) next.add(file.path);
                          else next.delete(file.path);
                          return next;
                        });
                      }}
                    />
                    <span className={`git-wb-file-mark mono tone-${markTone}`}>{file.mark}</span>
                    <span className="git-wb-file-path mono" title={file.path}>
                      {file.originalPath ? `${file.originalPath} → ${file.path}` : file.path}
                    </span>
                    {file.staged && (
                      <span className="git-wb-staged-tag">{t('git.workbench.stagedBadge')}</span>
                    )}
                    <span className="git-wb-file-actions" onClick={(event) => event.stopPropagation()}>
                      {file.unstaged && !file.conflicted && (
                        <button
                          type="button"
                          className="icon-button git-wb-row-icon"
                          data-testid={'git-stage-file-' + file.path}
                          title={t('git.workbench.stageFile')}
                          aria-label={t('git.workbench.stageFile')}
                          disabled={busy}
                          onClick={() => handleStageFile(file)}
                        >
                          <Icon name="plus" size={12} weight={2.2} />
                        </button>
                      )}
                      {file.staged && (
                        <button
                          type="button"
                          className="icon-button git-wb-row-icon"
                          data-testid={'git-unstage-file-' + file.path}
                          title={t('git.workbench.unstageFile')}
                          aria-label={t('git.workbench.unstageFile')}
                          disabled={busy}
                          onClick={() => handleUnstageFile(file)}
                        >
                          <Icon name="minus" size={12} weight={2.2} />
                        </button>
                      )}
                    </span>
                  </div>
                );
              })
            )}
          </div>
        </aside>

        <section className="git-wb-right-pane">
          <div className="git-wb-diff-card">
            {activeFile ? (
              <>
                <header className="git-wb-diff-head">
                  <Icon name="file-code" size={14} weight={1.9} />
                  <span className="git-wb-diff-path mono selectable" title={activeFile.path}>
                    {activeFile.path}
                  </span>
                  {diff && !diff.binary && (
                    <span className="git-wb-diff-counts mono">
                      <span className="tone-text-done">+{diff.added}</span>
                      <span className="tone-text-err">−{diff.removed}</span>
                    </span>
                  )}
                  <span className="spacer" />
                  {activeFile.staged && activeFile.unstaged && (
                    <div className="git-wb-diff-tabs" role="tablist">
                      <button
                        type="button"
                        role="tab"
                        aria-selected={!viewingStagedDiff}
                        className={'git-wb-diff-tab' + (!viewingStagedDiff ? ' active' : '')}
                        data-testid="git-diff-tab-unstaged"
                        onClick={() => setDiffStagedPreference(false)}
                      >
                        M
                      </button>
                      <button
                        type="button"
                        role="tab"
                        aria-selected={viewingStagedDiff}
                        className={'git-wb-diff-tab' + (viewingStagedDiff ? ' active' : '')}
                        data-testid="git-diff-tab-staged"
                        onClick={() => setDiffStagedPreference(true)}
                      >
                        {t('git.workbench.stagedBadge')}
                      </button>
                    </div>
                  )}
                </header>

                <div className="git-wb-diff-scroll" data-testid="git-diff-viewer">
                  {diffLoading ? (
                    <DialogLoading rows={3} />
                  ) : !diff || diff.binary || diff.hunks.length === 0 ? (
                    <div className="git-wb-empty">{t('git.workbench.diffEmpty')}</div>
                  ) : (
                    diff.hunks.map((hunk) => (
                      <div key={hunk.index} className="git-wb-hunk" data-testid={`git-hunk-${hunk.index}`}>
                        <div className="git-wb-hunk-bar">
                          <span className="git-wb-hunk-header mono">{hunk.header}</span>
                          {!activeFile.conflicted && (
                            <button
                              type="button"
                              className="btn git-wb-hunk-btn"
                              data-testid={`git-hunk-action-${hunk.index}`}
                              disabled={busy}
                              onClick={() =>
                                handleStageOrUnstageHunk(hunk.rawPatch, viewingStagedDiff)
                              }
                            >
                              <Icon
                                name={viewingStagedDiff ? 'minus' : 'plus'}
                                size={11}
                                weight={2.2}
                              />
                              <span>
                                {t(
                                  viewingStagedDiff
                                    ? 'git.workbench.unstageHunk'
                                    : 'git.workbench.stageHunk',
                                )}
                              </span>
                            </button>
                          )}
                        </div>
                        <div className="git-wb-hunk-lines mono">
                          {hunk.lines.map((line, lineIdx) => (
                            <div
                              key={lineIdx}
                              className={`git-wb-diff-line kind-${line.kind}`}
                            >
                              <span className="git-wb-gutter">
                                {line.oldLine ?? ''}
                              </span>
                              <span className="git-wb-gutter">
                                {line.newLine ?? ''}
                              </span>
                              <span className="git-wb-op">
                                {line.kind === 'add' ? '+' : line.kind === 'del' ? '-' : ' '}
                              </span>
                              <span className="git-wb-code selectable">{line.text}</span>
                            </div>
                          ))}
                        </div>
                      </div>
                    ))
                  )}
                </div>
              </>
            ) : (
              <div className="git-wb-empty">{t('git.workbench.diffEmpty')}</div>
            )}
          </div>

          <div className="git-wb-commit-box">
            <div className="git-wb-commit-head">
              <label className="git-wb-section-label" htmlFor="git-wb-commit-textarea">
                {t('git.commit.message')}
              </label>
              <button
                type="button"
                className="btn git-wb-compact-btn"
                data-testid="git-ai-draft-btn"
                disabled={
                  aiDrafting ||
                  busy ||
                  (orderedSelectedPaths.length === 0 && stagedCount === 0)
                }
                onClick={() => {
                  void handleDraftMessage();
                }}
              >
                <Icon name="sparkles" size={12} weight={2} />
                <span>{t(aiDrafting ? 'git.commit.aiDrafting' : 'git.commit.aiDraft')}</span>
              </button>
            </div>
            <textarea
              id="git-wb-commit-textarea"
              className="dialog-field git-wb-commit-textarea mono"
              data-testid="git-commit-message"
              placeholder={t('git.commit.message')}
              value={commitMessage}
              disabled={busy}
              rows={3}
              onChange={(event) => setCommitMessage(event.target.value)}
            />
          </div>
        </section>
      </div>
      </>)}
    </DialogShell>
  );

  return renderInBodyPortal(dialog);
}

export default GitWorkbenchPanel;

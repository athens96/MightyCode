// Unified Agent Context Editor (`CLAUDE.md` / `AGENTS.md` / `GEMINI.md` compare, edit & 1-click sync)
// + Multi-CLI MCP Hub (built-in runtime MCP visibility, Claude/Codex/Antigravity 1-click registration,
// OAuth 2.1 PKCE login, removal, toolkit.json persistence, and Claude Hooks inspector) (`MC-10`).
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useServices, useStoreSelector } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import {
  ALL_AGENT_PROVIDERS,
  PROVIDER_DISPLAY_NAMES,
  canSubmitMcpUpsert,
  computeFilesDiverged,
  computeLineDiff,
  expectedTexts,
  formatByteCount,
  overwrittenBySync,
  groupExternalMcpServers,
  isValidMcpHttpUrl,
  parseArgsInput,
  type AgentContextSnapshot,
  type AgentContextTab,
  type AgentProviderId,
  type BuiltinMcpServer,
  type ClaudeHookItem,
  type ExternalMcpServer,
  type GroupedExternalMcpServer,
  type InstructionFileItem,
  type McpScope,
  type McpTransport,
} from './agentContextModel';
import './AgentContextPanel.css';

export interface AgentContextPanelProps {
  open: boolean;
  workspaceId?: string | null;
  onClose: () => void;
}

const DEFAULT_FILE_SLOTS: readonly InstructionFileItem[] = [
  { provider: 'claude', name: 'CLAUDE.md', exists: false, text: '', byteLen: 0 },
  { provider: 'codex', name: 'AGENTS.md', exists: false, text: '', byteLen: 0 },
  { provider: 'gemini', name: 'GEMINI.md', exists: false, text: '', byteLen: 0 },
];

function renderInBodyPortal(node: ReactNode) {
  if (typeof document !== 'undefined' && document.body) {
    return createPortal(node, document.body);
  }
  return node;
}

function ProviderBadge({ provider }: { provider: string }) {
  const id = (
    provider === 'claude' || provider === 'codex' || provider === 'gemini' ? provider : 'claude'
  ) as AgentProviderId;
  return (
    <span className={`agent-ctx-provider-pill provider-${id}`}>
      <span className="agent-ctx-provider-dot" aria-hidden="true" />
      <span>{PROVIDER_DISPLAY_NAMES[id]}</span>
    </span>
  );
}

export function AgentContextPanel({
  open,
  workspaceId: propWorkspaceId,
  onClose,
}: AgentContextPanelProps) {
  const { bridge, t } = useServices();
  usePageOverlay(open);

  const fallbackWorkspaceId = useStoreSelector((state) => state.snapshot?.activeWorkspaceId ?? null);
  const workspaceId = propWorkspaceId !== undefined ? propWorkspaceId : fallbackWorkspaceId;

  const [tab, setTab] = useState<AgentContextTab>('instructions');
  const [workspacePath, setWorkspacePath] = useState<string>('');
  const [files, setFiles] = useState<InstructionFileItem[]>([...DEFAULT_FILE_SLOTS]);
  const [serverDiverged, setServerDiverged] = useState(false);
  const [builtinServers, setBuiltinServers] = useState<BuiltinMcpServer[]>([]);
  const [externalServers, setExternalServers] = useState<ExternalMcpServer[]>([]);
  const [claudeHooks, setClaudeHooks] = useState<ClaudeHookItem[]>([]);

  const [selectedFile, setSelectedFile] = useState<string>('CLAUDE.md');
  const [draftText, setDraftText] = useState<string>('');
  const [compareTarget, setCompareTarget] = useState<string>('');
  const [copyConfirmTarget, setCopyConfirmTarget] = useState<string | null>(null);
  const [copyMenuOpen, setCopyMenuOpen] = useState(false);
  const [syncConfirm, setSyncConfirm] = useState(false);
  const [removeConfirmKey, setRemoveConfirmKey] = useState<string | null>(null);

  // Add MCP Server form state
  const [mcpName, setMcpName] = useState('');
  const [mcpTransport, setMcpTransport] = useState<McpTransport>('http');
  const [mcpUrl, setMcpUrl] = useState('');
  const [mcpBearerEnv, setMcpBearerEnv] = useState('');
  const [mcpCommand, setMcpCommand] = useState('');
  const [mcpArgsText, setMcpArgsText] = useState('');
  const [mcpScope, setMcpScope] = useState<McpScope>('user');
  const [mcpProviders, setMcpProviders] = useState<AgentProviderId[]>(['claude', 'codex', 'gemini']);
  const [saveToToolkit, setSaveToToolkit] = useState(false);

  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [oauthBusyKey, setOauthBusyKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const applyPayload = useCallback(
    (res: AgentContextSnapshot | null | undefined, preserveSelected?: string, keepDraft = false) => {
      if (!res) return;
      const nextFiles = res.files && res.files.length > 0 ? res.files : [...DEFAULT_FILE_SLOTS];
      setWorkspacePath(res.workspacePath ?? '');
      setFiles(nextFiles);
      setServerDiverged(Boolean(res.diverged));
      setBuiltinServers(res.builtinMcpServers ?? []);
      setExternalServers(res.externalMcpServers ?? []);
      setClaudeHooks(res.claudeHooks ?? []);

      const targetName = preserveSelected ?? selectedFile;
      const currentEntry = nextFiles.find((f) => f.name === targetName);
      const chosen =
        currentEntry && (preserveSelected || currentEntry.exists)
          ? currentEntry
          : nextFiles.find((f) => f.exists) ?? nextFiles[0]!;
      // An MCP change or a sign-in reloads the panel: the instruction file being edited keeps its unsaved words.
      if (keepDraft) return;
      setSelectedFile(chosen.name);
      setDraftText(chosen.text ?? '');
    },
    [selectedFile],
  );

  const reload = useCallback(
    async (showSkeleton: boolean, keepFileName?: string, keepDraft = false) => {
      if (showSkeleton) setLoading(true);
      try {
        const res = await bridge.call<AgentContextSnapshot>('agentContextGet', { workspaceId });
        applyPayload(res, keepFileName, keepDraft);
        setError(null);
      } catch (reason: unknown) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        if (showSkeleton) setLoading(false);
      }
    },
    [bridge, workspaceId, applyPayload],
  );

  useEffect(() => {
    if (!open) {
      setCopyConfirmTarget(null);
      setCopyMenuOpen(false);
      setNote(null);
      setError(null);
      return undefined;
    }
    let live = true;
    setLoading(true);
    setError(null);
    setNote(null);
    void bridge
      .call<AgentContextSnapshot>('agentContextGet', { workspaceId })
      .then(
        (res) => {
          if (!live) return;
          if (res) {
            const nextFiles = res.files && res.files.length > 0 ? res.files : [...DEFAULT_FILE_SLOTS];
            setWorkspacePath(res.workspacePath ?? '');
            setFiles(nextFiles);
            setServerDiverged(Boolean(res.diverged));
            setBuiltinServers(res.builtinMcpServers ?? []);
            setExternalServers(res.externalMcpServers ?? []);
            setClaudeHooks(res.claudeHooks ?? []);
            const firstExisting = nextFiles.find((f) => f.exists) ?? nextFiles[0]!;
            setSelectedFile(firstExisting.name);
            setDraftText(firstExisting.text ?? '');
          }
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
  }, [open, workspaceId, bridge]);

  const currentFile: InstructionFileItem = useMemo(
    () => files.find((f) => f.name === selectedFile) ?? files[0] ?? DEFAULT_FILE_SLOTS[0]!,
    [files, selectedFile],
  );

  const edited = draftText !== (currentFile.text ?? '');

  const diverged = useMemo(
    () => serverDiverged || computeFilesDiverged(files),
    [serverDiverged, files],
  );

  const otherFiles = useMemo(
    () => files.filter((f) => f.name !== currentFile.name),
    [files, currentFile.name],
  );

  const compareFile: InstructionFileItem | null = useMemo(
    () => (compareTarget ? otherFiles.find((f) => f.name === compareTarget) ?? null : null),
    [compareTarget, otherFiles],
  );

  const diffLines = useMemo(
    () => (compareFile ? computeLineDiff(draftText, compareFile.text ?? '') : []),
    [compareFile, draftText],
  );

  const groupedServers: GroupedExternalMcpServer[] = useMemo(
    () => groupExternalMcpServers(externalServers),
    [externalServers],
  );

  const formValid = useMemo(
    () =>
      canSubmitMcpUpsert({
        name: mcpName,
        transport: mcpTransport,
        url: mcpUrl,
        bearerTokenEnvVar: mcpBearerEnv,
        command: mcpCommand,
        argsText: mcpArgsText,
        scope: mcpScope,
        providers: mcpProviders,
        saveToToolkit,
      }),
    [
      mcpName,
      mcpTransport,
      mcpUrl,
      mcpBearerEnv,
      mcpCommand,
      mcpArgsText,
      mcpScope,
      mcpProviders,
      saveToToolkit,
    ],
  );

  if (!open) return null;

  const selectInstructionTab = (name: string) => {
    const target = files.find((f) => f.name === name);
    setSelectedFile(name);
    setDraftText(target?.text ?? '');
    if (compareTarget === name) {
      setCompareTarget('');
    }
    setCopyConfirmTarget(null);
    setCopyMenuOpen(false);
    setError(null);
    setNote(null);
  };

  const handleSaveCurrentFile = async () => {
    if (busy || !edited) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const keep = currentFile.name;
      const res = await bridge.call<AgentContextSnapshot>('agentContextWriteFile', {
        workspaceId,
        name: keep,
        text: draftText,
        // What the file held when it was read: the host refuses to write over a change made on disk since (an agent's).
        expected: { [keep]: currentFile.exists ? currentFile.text ?? '' : null },
      });
      if (res?.files) {
        applyPayload(res, keep);
      } else {
        await reload(false, keep);
      }
      setNote(t('agentContext.saved', { name: keep }));
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleSyncAllFiles = async () => {
    if (busy || draftText.trim().length === 0) return;
    setSyncConfirm(false);
    setBusy(true);
    setError(null);
    setNote(null);
    setCopyConfirmTarget(null);
    setCopyMenuOpen(false);
    try {
      const keep = currentFile.name;
      const res = await bridge.call<AgentContextSnapshot>('agentContextSyncFiles', {
        workspaceId,
        sourceName: keep,
        content: draftText,
        expected: expectedTexts(files),
      });
      if (res?.files) {
        applyPayload(res, keep);
      } else {
        await reload(false, keep);
      }
      setNote(t('agentContext.synced'));
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleCopyToSingleTarget = async (targetName: string) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNote(null);
    setCopyConfirmTarget(null);
    setCopyMenuOpen(false);
    try {
      const keep = currentFile.name;
      const res = await bridge.call<AgentContextSnapshot>('agentContextSyncFiles', {
        workspaceId,
        sourceName: keep,
        content: draftText,
        targetNames: [targetName],
        expected: expectedTexts(files),
      });
      if (res?.files) {
        applyPayload(res, keep);
      } else {
        await reload(false, keep);
      }
      setNote(t('agentContext.saved', { name: targetName }));
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const toggleMcpProvider = (provider: AgentProviderId) => {
    setMcpProviders((prev) =>
      prev.includes(provider) ? prev.filter((item) => item !== provider) : [...prev, provider],
    );
  };

  const handleRegisterMcp = async () => {
    if (busy || !formValid) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const payload: Record<string, unknown> = {
        workspaceId,
        name: mcpName.trim(),
        transport: mcpTransport,
        scope: mcpScope,
        providers: mcpProviders,
        saveToToolkit,
      };
      if (mcpTransport === 'http') {
        payload.url = mcpUrl.trim();
        if (mcpBearerEnv.trim().length > 0) {
          payload.bearerTokenEnvVar = mcpBearerEnv.trim();
        }
      } else {
        payload.command = mcpCommand.trim();
        payload.args = parseArgsInput(mcpArgsText);
      }
      const submittedName = mcpName.trim();
      const res = await bridge.call<AgentContextSnapshot>('agentContextMcpUpsert', payload);
      if (res?.files) {
        applyPayload(res, currentFile.name, true);
      } else {
        await reload(false, currentFile.name, true);
      }
      setMcpName('');
      setMcpUrl('');
      setMcpBearerEnv('');
      setMcpCommand('');
      setMcpArgsText('');
      setNote(
        saveToToolkit
          ? t('agentContext.savedToolkit')
          : t('agentContext.saved', { name: submittedName }),
      );
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleRemoveServer = async (server: GroupedExternalMcpServer) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const res = await bridge.call<AgentContextSnapshot>('agentContextMcpRemove', {
        workspaceId,
        name: server.name,
        scope: server.scope,
        providers: server.providers,
      });
      if (res?.files) {
        applyPayload(res, currentFile.name, true);
      } else {
        await reload(false, currentFile.name, true);
      }
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleOAuthLogin = async (name: string, url: string, busyKey: string) => {
    if (busy || oauthBusyKey || !isValidMcpHttpUrl(url)) return;
    setOauthBusyKey(busyKey);
    setError(null);
    setNote(null);
    try {
      await bridge.call('agentContextMcpOAuthLogin', { name, url });
      await reload(false, currentFile.name, true);
      setNote(t('agentContext.oauthConnected'));
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setOauthBusyKey(null);
    }
  };

  const statusNode = error ? (
    <span className="tone-text-err selectable" data-testid="agent-context-error">
      {error}
    </span>
  ) : note ? (
    <span className="tone-text-done selectable" data-testid="agent-context-note">
      {note}
    </span>
  ) : undefined;

  const headerTools = (
    <button
      type="button"
      className="icon-button"
      data-testid="agent-context-refresh"
      aria-label={t('agentContext.open')}
      title={t('agentContext.open')}
      disabled={loading || busy || Boolean(oauthBusyKey)}
      onClick={() => {
        void reload(false, currentFile.name, edited);
      }}
    >
      <Icon name="refresh-cw" size={14} weight={2} />
    </button>
  );

  const dialog = (
    <DialogShell
      kind="panel"
      panel={'agentContext' as never}
      width={960}
      height={640}
      minWidth={660}
      minHeight={440}
      label={t('agentContext.title')}
      title={t('agentContext.title')}
      subtitle={workspacePath ? <span className="mono">{workspacePath}</span> : undefined}
      icon={<Icon name="brain" size={18} weight={1.8} />}
      tools={headerTools}
      busy={busy || Boolean(oauthBusyKey)}
      onCancel={onClose}
      testId="agent-context-panel"
      closeTestId="agent-context-close"
      bodyClassName="agent-ctx-body"
      backdrop={{ 'data-page-overlay': '' }}
      status={statusNode}
    >
      <div className="agent-ctx-topbar">
        <div className="agent-ctx-tabs" role="tablist" aria-label={t('agentContext.title')}>
          <button
            type="button"
            role="tab"
            aria-selected={tab === 'instructions'}
            className={'agent-ctx-tab' + (tab === 'instructions' ? ' active' : '')}
            data-testid="agent-context-tab-instructions"
            onClick={() => setTab('instructions')}
          >
            <Icon name="file-text" size={13} weight={2} />
            <span>{t('agentContext.tab.instructions')}</span>
            {diverged && (
              <span className="agent-ctx-warn-dot" aria-hidden="true">
                <Icon name="triangle-alert" size={12} weight={2.2} />
              </span>
            )}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === 'mcp'}
            className={'agent-ctx-tab' + (tab === 'mcp' ? ' active' : '')}
            data-testid="agent-context-tab-mcp"
            onClick={() => setTab('mcp')}
          >
            <Icon name="network" size={13} weight={2} />
            <span>{t('agentContext.tab.mcp')}</span>
            <span className="agent-ctx-count-pill">
              {t('agentContext.servers', { count: groupedServers.length })}
            </span>
          </button>
        </div>
      </div>

      {loading ? (
        <DialogLoading rows={4} />
      ) : tab === 'instructions' ? (
        <div className="agent-ctx-instructions">
          <div className="agent-ctx-file-bar">
            <div className="agent-ctx-file-segments" role="tablist">
              {files.map((file) => {
                const isSelected = file.name === currentFile.name;
                return (
                  <button
                    key={file.name}
                    type="button"
                    role="tab"
                    aria-selected={isSelected}
                    className={'agent-ctx-file-seg' + (isSelected ? ' active' : '')}
                    data-testid={'agent-context-file-' + file.name}
                    onClick={() => selectInstructionTab(file.name)}
                  >
                    <ProviderBadge provider={file.provider} />
                    <span className="mono agent-ctx-file-name">{file.name}</span>
                    <span className={'agent-ctx-file-meta' + (!file.exists ? ' missing' : '')}>
                      {file.exists ? formatByteCount(file.byteLen) : t('agentContext.missing')}
                    </span>
                  </button>
                );
              })}
            </div>

            <div className="agent-ctx-compare-box">
              <Icon name="columns-2" size={13} weight={2} />
              <label className="agent-ctx-compare-label" htmlFor="agent-ctx-compare-select">
                {t('agentContext.compareWith', { name: currentFile.name })}
              </label>
              <select
                id="agent-ctx-compare-select"
                className="dialog-field agent-ctx-compare-select mono"
                data-testid="agent-context-compare-select"
                value={compareTarget}
                onChange={(event) => setCompareTarget(event.target.value)}
              >
                <option value="">{t('agentContext.none')}</option>
                {otherFiles.map((file) => (
                  <option key={file.name} value={file.name}>
                    {file.name}
                    {!file.exists ? ` (${t('agentContext.missing')})` : ''}
                  </option>
                ))}
              </select>
            </div>
          </div>

          {diverged && (
            <div className="agent-ctx-diverged-banner" role="status" data-testid="agent-context-diverged">
              <div className="agent-ctx-diverged-msg">
                <Icon name="triangle-alert" size={14} weight={2} />
                <span>{t('agentContext.diverged')}</span>
              </div>
              <button
                type="button"
                className="btn small primary"
                data-testid="agent-context-banner-sync"
                disabled={busy || draftText.trim().length === 0}
                onClick={() => {
                  // The files that differ are overwritten: said first, written only on the confirm.
                  if (overwrittenBySync(files, currentFile.name, draftText).length === 0) void handleSyncAllFiles();
                  else setSyncConfirm(true);
                }}
              >
                <Icon name="copy" size={12} weight={2} />
                <span>{t('agentContext.syncAll')}</span>
              </button>
            </div>
          )}

          {syncConfirm && (
            <div className="agent-ctx-confirm-bar" role="alert" data-testid="agent-context-sync-confirm">
              <span>{t('agentContext.syncConfirm', { files: overwrittenBySync(files, currentFile.name, draftText).join(', ') })}</span>
              <div className="agent-ctx-confirm-actions">
                <button type="button" className="btn small" onClick={() => setSyncConfirm(false)}>
                  <Icon name="x" size={12} weight={2} />
                  <span>{t('dialog.close')}</span>
                </button>
                <button type="button" className="btn small danger" data-testid="agent-context-confirm-sync" disabled={busy}
                  onClick={() => { void handleSyncAllFiles(); }}>
                  <span>{t('agentContext.copy.overwrite')}</span>
                </button>
              </div>
            </div>
          )}

          {copyConfirmTarget && (
            <div className="agent-ctx-confirm-bar" role="alert" data-testid="agent-context-copy-confirm">
              <span>
                {t('agentContext.copy.confirm', {
                  source: currentFile.name,
                  target: copyConfirmTarget,
                })}
              </span>
              <div className="agent-ctx-confirm-actions">
                <button
                  type="button"
                  className="btn small"
                  onClick={() => setCopyConfirmTarget(null)}
                >
                  <Icon name="x" size={12} weight={2} />
                  <span>{t('dialog.close')}</span>
                </button>
                <button
                  type="button"
                  className="btn small danger"
                  data-testid="agent-context-confirm-overwrite"
                  disabled={busy}
                  onClick={() => {
                    void handleCopyToSingleTarget(copyConfirmTarget);
                  }}
                >
                  <span>{t('agentContext.copy.overwrite')}</span>
                </button>
              </div>
            </div>
          )}

          <div className={'agent-ctx-editor-split' + (compareFile ? ' comparing' : '')}>
            <div className="agent-ctx-editor-pane">
              <textarea
                className="agent-ctx-textarea mono"
                data-autofocus
                data-testid="agent-context-editor"
                aria-label={currentFile.name}
                value={draftText}
                spellCheck={false}
                onChange={(event) => {
                  setDraftText(event.target.value);
                  setNote(null);
                }}
              />
            </div>

            {compareFile && (
              <div className="agent-ctx-diff-pane" data-testid="agent-context-diff-view">
                <div className="agent-ctx-diff-head mono">
                  <span>{currentFile.name}</span>
                  <span>↔</span>
                  <span>{compareFile.name}</span>
                </div>
                <div className="agent-ctx-diff-scroll mono selectable">
                  {diffLines.length === 0 ? (
                    <div className="agent-ctx-empty">{t('agentContext.none')}</div>
                  ) : (
                    diffLines.map((line, index) => (
                      <div key={index} className={`agent-ctx-diff-line kind-${line.kind}`}>
                        <span className="agent-ctx-diff-num">{line.leftLine ?? ''}</span>
                        <span className="agent-ctx-diff-num">{line.rightLine ?? ''}</span>
                        <span className="agent-ctx-diff-sign">
                          {line.kind === 'remove' ? '-' : line.kind === 'add' ? '+' : ' '}
                        </span>
                        <span className="agent-ctx-diff-text">{line.text}</span>
                      </div>
                    ))
                  )}
                </div>
              </div>
            )}
          </div>

          <div className="agent-ctx-editor-foot">
            <div className="agent-ctx-foot-left">
              <div className="agent-ctx-copy-wrap">
                <button
                  type="button"
                  className="btn"
                  data-testid="agent-context-copy-menu-btn"
                  disabled={busy || draftText.trim().length === 0 || edited}
                  title={edited ? t('agentContext.copy.saveFirst') : t('agentContext.copy.help')}
                  onClick={() => setCopyMenuOpen((prev) => !prev)}
                >
                  <Icon name="copy" size={13} weight={2} />
                  <span>{t('agentContext.copy.menu')}</span>
                </button>
                {copyMenuOpen && !edited && (
                  <div className="agent-ctx-copy-menu" role="menu">
                    {otherFiles.map((target) => (
                      <button
                        key={target.name}
                        type="button"
                        role="menuitem"
                        className="agent-ctx-copy-item"
                        data-testid={'agent-context-copy-to-' + target.name}
                        onClick={() => {
                          setCopyMenuOpen(false);
                          setCopyConfirmTarget(target.name);
                        }}
                      >
                        {t('agentContext.copy.to', { target: target.name })}
                      </button>
                    ))}
                  </div>
                )}
              </div>

              <button
                type="button"
                className="btn"
                data-testid="agent-context-sync-all"
                disabled={busy || draftText.trim().length === 0}
                title={t('agentContext.copy.help')}
                onClick={() => {
                  // The files that differ are overwritten: said first, written only on the confirm.
                  if (overwrittenBySync(files, currentFile.name, draftText).length === 0) void handleSyncAllFiles();
                  else setSyncConfirm(true);
                }}
              >
                <Icon name="check" size={13} weight={2} />
                <span>{t('agentContext.syncAll')}</span>
              </button>
            </div>

            <div className="agent-ctx-foot-right">
              <button
                type="button"
                className="btn"
                data-testid="agent-context-revert"
                disabled={busy || !edited}
                onClick={() => {
                  setDraftText(currentFile.text ?? '');
                  setNote(null);
                }}
              >
                <Icon name="undo-2" size={13} weight={2} />
                <span>{t('agentContext.revert')}</span>
              </button>
              <button
                type="button"
                className="btn primary"
                data-testid="agent-context-save"
                disabled={busy || !edited}
                onClick={() => {
                  void handleSaveCurrentFile();
                }}
              >
                <Icon name="check" size={13} weight={2} />
                <span>{t('savedPrompts.save')}</span>
              </button>
            </div>
          </div>
        </div>
      ) : (
        <div className="agent-ctx-mcp-columns">
          <div className="agent-ctx-mcp-left">
            {/* Built-in System MCP Servers */}
            <section className="agent-ctx-section" data-testid="agent-context-builtin-section">
              <header className="agent-ctx-section-head">
                <div className="agent-ctx-section-title">
                  <Icon name="shield" size={14} weight={2} />
                  <strong>{t('agentContext.builtinTitle')}</strong>
                </div>
              </header>
              <p className="agent-ctx-section-note">{t('agentContext.builtinNote')}</p>
              <div className="agent-ctx-builtin-grid">
                {builtinServers.map((srv) => (
                  <div
                    key={srv.name}
                    className={'agent-ctx-builtin-card' + (srv.active ? ' active' : '')}
                    data-testid={'builtin-mcp-' + srv.name}
                  >
                    <div className="agent-ctx-card-top">
                      <span className="mono agent-ctx-srv-name">{srv.name}</span>
                      <span className={'agent-ctx-state-pill' + (srv.active ? ' active' : ' inactive')}>
                        <Icon name={srv.active ? 'circle-check' : 'wrench'} size={11} weight={2} />
                        <span>
                          {srv.active ? t('agentContext.builtinActive') : t('agentContext.builtinInactive')}
                        </span>
                      </span>
                    </div>
                    <div className="agent-ctx-card-providers">
                      {srv.providers.map((p) => (
                        <ProviderBadge key={p} provider={p} />
                      ))}
                      <span className="agent-ctx-tool-count mono">×{srv.toolCount}</span>
                    </div>
                  </div>
                ))}
              </div>
            </section>

            {/* External Multi-CLI MCP Servers */}
            <section className="agent-ctx-section" data-testid="agent-context-external-section">
              <header className="agent-ctx-section-head">
                <div className="agent-ctx-section-title">
                  <Icon name="puzzle" size={14} weight={2} />
                  <strong>{t('agentContext.externalTitle')}</strong>
                </div>
                <span className="agent-ctx-count-pill">
                  {t('agentContext.servers', { count: groupedServers.length })}
                </span>
              </header>

              {groupedServers.length === 0 ? (
                <div className="agent-ctx-empty" data-testid="agent-context-external-empty">
                  {t('agentContext.none')}
                </div>
              ) : (
                <div className="agent-ctx-server-list">
                  {groupedServers.map((srv) => {
                    const isHttp = srv.transport === 'http' && Boolean(srv.url);
                    const isOAuthBusy = oauthBusyKey === srv.key;
                    return (
                      <div
                        key={srv.key}
                        className="agent-ctx-server-card"
                        data-testid={'external-mcp-' + srv.key}
                      >
                        <div className="agent-ctx-card-top">
                          <div className="agent-ctx-srv-heading">
                            <Icon name={isHttp ? 'globe' : 'terminal'} size={13} weight={2} />
                            <strong className="mono agent-ctx-srv-name">{srv.name}</strong>
                            <span className="agent-ctx-scope-pill">
                              {srv.scope === 'project'
                                ? t('agentContext.scope.project')
                                : t('agentContext.scope.user')}
                            </span>
                            <span className="agent-ctx-transport-pill">
                              {isHttp
                                ? t('agentContext.transport.http')
                                : t('agentContext.transport.stdio')}
                            </span>
                          </div>

                          <div className="agent-ctx-srv-actions">
                            {isHttp && srv.url && (
                              srv.oauthConnected ? (
                                <span
                                  className="agent-ctx-oauth-pill connected"
                                  data-testid={'mcp-oauth-connected-' + srv.name}
                                >
                                  <Icon name="circle-check" size={11} weight={2} />
                                  <span>{t('agentContext.oauthConnected')}</span>
                                </span>
                              ) : (
                                <button
                                  type="button"
                                  className="btn small"
                                  data-testid={'mcp-oauth-login-' + srv.name}
                                  disabled={busy || Boolean(oauthBusyKey)}
                                  onClick={() => {
                                    void handleOAuthLogin(srv.name, srv.url!, srv.key);
                                  }}
                                >
                                  <Icon name="globe" size={12} weight={2} />
                                  <span>
                                    {isOAuthBusy ? t('dialog.loading') : t('agentContext.oauthLogin')}
                                  </span>
                                </button>
                              )
                            )}
                            <button
                              type="button"
                              className="btn small"
                              data-testid={'mcp-remove-' + srv.key}
                              aria-label={t('agentContext.removeServer')}
                              title={removeConfirmKey === srv.key ? t('agentContext.removeConfirm', { name: srv.name, providers: srv.providers.join(', '), target: srv.target }) : t('agentContext.removeServer')}
                              disabled={busy || Boolean(oauthBusyKey)}
                              onClick={() => {
                                // Removing changes every agent's file this row stands for: the first click says so, the second removes.
                                if (removeConfirmKey !== srv.key) {
                                  setRemoveConfirmKey(srv.key);
                                  return;
                                }
                                setRemoveConfirmKey(null);
                                void handleRemoveServer(srv);
                              }}
                            >
                              <Icon name="trash" size={12} weight={2} />
                              <span>{removeConfirmKey === srv.key ? t('agentContext.removeConfirm', { name: srv.name, providers: srv.providers.join(', '), target: srv.target }) : t('agentContext.removeServer')}</span>
                            </button>
                          </div>
                        </div>

                        <div className="agent-ctx-srv-target mono selectable" title={srv.target}>
                          {srv.target}
                        </div>

                        <div className="agent-ctx-srv-foot">
                          <div className="agent-ctx-card-providers">
                            {srv.providers.map((p) => (
                              <ProviderBadge key={p} provider={p} />
                            ))}
                          </div>
                          <span className="agent-ctx-srv-sources mono">{srv.sources.join(' · ')}</span>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </section>

            {/* Claude Hooks (Read-Only Inspector) */}
            <section className="agent-ctx-section" data-testid="agent-context-hooks-section">
              <header className="agent-ctx-section-head">
                <div className="agent-ctx-section-title">
                  <Icon name="wrench" size={14} weight={2} />
                  <strong>{t('agentContext.hooks', { count: claudeHooks.length })}</strong>
                </div>
              </header>

              {claudeHooks.length === 0 ? (
                <div className="agent-ctx-empty">{t('agentContext.none')}</div>
              ) : (
                <div className="agent-ctx-hook-list">
                  {claudeHooks.map((hook, idx) => (
                    <div key={`${hook.scope}:${hook.event}:${idx}`} className="agent-ctx-hook-card">
                      <div className="agent-ctx-card-top">
                        <strong className="mono agent-ctx-srv-name">{hook.event}</strong>
                        <span className="agent-ctx-srv-sources mono">{hook.source}</span>
                      </div>
                      {hook.commands.map((cmd, cmdIdx) => (
                        <div key={cmdIdx} className="agent-ctx-srv-target mono selectable">
                          {cmd}
                        </div>
                      ))}
                    </div>
                  ))}
                </div>
              )}
              <p className="agent-ctx-readonly-note">{t('agentContext.readOnly')}</p>
            </section>
          </div>

          {/* Right Pane: One-Click Multi-CLI MCP Register Form */}
          <aside className="agent-ctx-add-card" data-testid="agent-context-add-form">
            <header className="agent-ctx-section-head">
              <div className="agent-ctx-section-title">
                <Icon name="plus" size={14} weight={2.2} />
                <strong>{t('agentContext.addTitle')}</strong>
              </div>
            </header>

            <div className="agent-ctx-seg-row" role="radiogroup" aria-label={t('agentContext.addTitle')}>
              <button
                type="button"
                className={'agent-ctx-seg-btn' + (mcpTransport === 'http' ? ' active' : '')}
                data-testid="mcp-transport-http"
                onClick={() => setMcpTransport('http')}
              >
                <Icon name="globe" size={12} weight={2} />
                <span>{t('agentContext.transport.http')}</span>
              </button>
              <button
                type="button"
                className={'agent-ctx-seg-btn' + (mcpTransport === 'stdio' ? ' active' : '')}
                data-testid="mcp-transport-stdio"
                onClick={() => setMcpTransport('stdio')}
              >
                <Icon name="terminal" size={12} weight={2} />
                <span>{t('agentContext.transport.stdio')}</span>
              </button>
            </div>

            <div className="agent-ctx-form-field">
              <label htmlFor="agent-ctx-mcp-name">{t('agentContext.field.name')}</label>
              <input
                id="agent-ctx-mcp-name"
                type="text"
                className="dialog-field mono"
                data-testid="mcp-field-name"
                placeholder="context7"
                value={mcpName}
                onChange={(event) => setMcpName(event.target.value)}
              />
            </div>

            {mcpTransport === 'http' ? (
              <>
                <div className="agent-ctx-form-field">
                  <label htmlFor="agent-ctx-mcp-url">{t('agentContext.field.url')}</label>
                  <input
                    id="agent-ctx-mcp-url"
                    type="url"
                    className="dialog-field mono"
                    data-testid="mcp-field-url"
                    placeholder="https://example.com/mcp"
                    value={mcpUrl}
                    onChange={(event) => setMcpUrl(event.target.value)}
                  />
                </div>
                <div className="agent-ctx-form-field">
                  <label htmlFor="agent-ctx-mcp-bearer">{t('agentContext.field.bearerEnv')}</label>
                  <input
                    id="agent-ctx-mcp-bearer"
                    type="text"
                    className="dialog-field mono"
                    data-testid="mcp-field-bearer-env"
                    placeholder="MCP_API_TOKEN"
                    value={mcpBearerEnv}
                    onChange={(event) => setMcpBearerEnv(event.target.value)}
                  />
                </div>
              </>
            ) : (
              <>
                <div className="agent-ctx-form-field">
                  <label htmlFor="agent-ctx-mcp-cmd">{t('agentContext.field.command')}</label>
                  <input
                    id="agent-ctx-mcp-cmd"
                    type="text"
                    className="dialog-field mono"
                    data-testid="mcp-field-command"
                    placeholder="npx"
                    value={mcpCommand}
                    onChange={(event) => setMcpCommand(event.target.value)}
                  />
                </div>
                <div className="agent-ctx-form-field">
                  <label htmlFor="agent-ctx-mcp-args">{t('agentContext.field.args')}</label>
                  <input
                    id="agent-ctx-mcp-args"
                    type="text"
                    className="dialog-field mono"
                    data-testid="mcp-field-args"
                    placeholder="-y @upstash/context7-mcp"
                    value={mcpArgsText}
                    onChange={(event) => setMcpArgsText(event.target.value)}
                  />
                </div>
              </>
            )}

            <div className="agent-ctx-seg-row" role="radiogroup">
              <button
                type="button"
                className={'agent-ctx-seg-btn' + (mcpScope === 'user' ? ' active' : '')}
                data-testid="mcp-scope-user"
                onClick={() => setMcpScope('user')}
              >
                <span>{t('agentContext.scope.user')}</span>
              </button>
              <button
                type="button"
                className={'agent-ctx-seg-btn' + (mcpScope === 'project' ? ' active' : '')}
                data-testid="mcp-scope-project"
                onClick={() => setMcpScope('project')}
              >
                <span>{t('agentContext.scope.project')}</span>
              </button>
            </div>

            <div className="agent-ctx-form-field">
              <span>{t('agentContext.targets')}</span>
              <div className="agent-ctx-target-checks">
                {ALL_AGENT_PROVIDERS.map((provider) => {
                  const checked = mcpProviders.includes(provider);
                  return (
                    <label key={provider} className={'agent-ctx-target-check' + (checked ? ' checked' : '')}>
                      <input
                        type="checkbox"
                        data-testid={'mcp-target-' + provider}
                        checked={checked}
                        onChange={() => toggleMcpProvider(provider)}
                      />
                      <ProviderBadge provider={provider} />
                    </label>
                  );
                })}
              </div>
            </div>

            <label className="agent-ctx-toolkit-check">
              <input
                type="checkbox"
                data-testid="mcp-save-toolkit"
                checked={saveToToolkit}
                onChange={(event) => setSaveToToolkit(event.target.checked)}
              />
              <span>{t('agentContext.saveToolkit')}</span>
            </label>

            <div className="agent-ctx-add-actions">
              {mcpTransport === 'http' && isValidMcpHttpUrl(mcpUrl) && mcpName.trim().length > 0 && (
                <button
                  type="button"
                  className="btn"
                  data-testid="mcp-form-oauth-login"
                  disabled={busy || Boolean(oauthBusyKey)}
                  onClick={() => {
                    void handleOAuthLogin(mcpName.trim(), mcpUrl.trim(), 'form');
                  }}
                >
                  <Icon name="globe" size={13} weight={2} />
                  <span>{t('agentContext.oauthLogin')}</span>
                </button>
              )}
              <button
                type="button"
                className="btn primary agent-ctx-register-btn"
                data-testid="mcp-register-submit"
                disabled={busy || !formValid}
                onClick={() => {
                  void handleRegisterMcp();
                }}
              >
                <Icon name="plus" size={13} weight={2.2} />
                <span>{t('agentContext.registerButton')}</span>
              </button>
            </div>
          </aside>
        </div>
      )}
    </DialogShell>
  );

  return renderInBodyPortal(dialog);
}

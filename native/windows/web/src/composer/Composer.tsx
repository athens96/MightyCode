import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ClipboardEvent, type DragEvent, type KeyboardEvent } from 'react';
import { atDraft, onFirstLine, onLastLine, requestHistory, walkDown, walkUp, type HistoryWalk } from './history';
import { shallowEqual, useServices, useStoreSelector } from '../app/context';
import { ComposerCard } from '../cards/Cards';
import type { RunSession, Workspace } from '../store/models';
import { attachmentRefusal, byteLabel, toBase64 } from './attachments';
import { enterAction, maximumDraft, secondSendWords } from './keys';
import { choose, closed, move, paletteDraft, paletteKey, paletteState, safeIndex, type PaletteState, type SlashCatalog, type SlashCommand } from './slash';
import { BusySendPill, MenuList, SendMenu, StyleRow, Toolbar, usePopover } from './Toolbar';
import { Guided } from './Guided';
import { editedWords, queueMoveTarget, queueRowButtons, readRunNow, remoteRunNowTip, remoteRunNowWaits, rowAgent, runNowBranches, runNowTip, runNowWaits, type QueueRowButtons, type RunNow } from './queue';
import { Icon } from '../icons/Icon';
import type { AttachmentPreview, ComposerState, QueueItem, SendResult, WorktreeAgent } from './types';
import { ProviderMark } from '../conversation/ProviderMark';
import { usePageOverlay } from '../shell/overlay';
import { hostCapabilities } from '../slot/host';
import { newClientId, newPendingRequest } from '../pending/requests';
import { Motion } from '../theme/tokens';
import { appendingBlock } from '../mighty/runChangesModel';
import { NoticeChip } from '../conversation/NoticeChip';
import { parseDelegationNotice } from '../conversation/delegationNotice';
import { VoiceMicButton, VoiceStatusBar, useComposerVoice } from './VoiceInput';
import { insertDictatedText } from './voiceModel';
import { applyWorkspaceMention, filterMentionWorkspaces, mentionQuery, workspaceFolderName, workspaceMentionSlug } from './workspaceMention';
import { SavedPromptsPanel } from './SavedPromptsPanel';

/** The host on the Mac, where ⌘Enter is the second send (shell/menu.ts `macHost`). */
const onMac = () => hostCapabilities()?.platform === 'macos';

/** The editor grows to six lines, then scrolls (MainWindow.cs: ComposerLine + 5 × ComposerLineStep). */
const maxLines = 6;
/** The scanned catalogue is read again after this long (SlashCatalogCache, 30 s). */
const catalogLifetime = 30_000;
/** Typing is mirrored to the host once it rests this long (natural submit B4); a send, a pane switch or closing writes at once. */
const draftRest = 300;

/**
 * The agent pane's composer (spec §3). The draft lives here and is mirrored to the host once typing rests (`updateDraft`);
 * what may happen to it (send, queue, stop, a block) is the host's composer state, pushed when it changes. Enter empties the
 * editor at once and the views draw the request with 전송 중 before the host answers (natural submit B1, src/pending).
 */
export function Composer({ session }: { session: RunSession }) {
  const { bridge, store, t } = useServices();
  const { state, fill, workspaces, crossWorkspaceOn } = useStoreSelector(
    (stored) => ({
      state: stored.composers[session.id],
      fill: stored.fills[session.id],
      workspaces: stored.snapshot?.workspaces,
      crossWorkspaceOn: stored.snapshot?.crossWorkspaceLookup !== false,
    }),
    shallowEqual,
  );
  const [text, setText] = useState(() => session.draft ?? '');
  const [caret, setCaret] = useState(() => (session.draft ?? '').length);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [dismissedMention, setDismissedMention] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<AttachmentPreview | null>(null);
  const [dropping, setDropping] = useState(false);
  /** What the last send did (SendResult.outcome), on the card for the web smoke. */
  const [outcome, setOutcome] = useState<string | null>(null);
  const editor = useRef<HTMLTextAreaElement>(null);
  const sending = useRef(false);
  /** The draft not yet written to the host, and its timer. */
  const draftTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const unsaved = useRef<string | null>(null);
  /** The editor was typed in since the last send (a failed send gives its words back only to an editor left alone). */
  const edited = useRef(false);
  /** MC-106: where ↑/↓ stand among the pane's earlier requests; typing (or a send) starts over from the words being typed. */
  const walk = useRef<HistoryWalk>(atDraft);
  /** The queue row flashes when a request joins it (B3). */
  const [flashing, setFlashing] = useState(false);
  const flashTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [catalog, setCatalog] = useState<{ value: SlashCatalog; read: number } | null>(null);
  const [dismissedFor, setDismissedFor] = useState<string | null>(null);
  const [palette, setPalette] = useState<PaletteState>(closed);
  const activeMention = session.kind !== 'shell' && crossWorkspaceOn && palette.commands.length === 0
    ? mentionQuery(text, caret)
    : null;
  const mentionKey = activeMention ? `${activeMention.start}:${activeMention.query}` : null;
  const mentionRows = activeMention && mentionKey !== dismissedMention && workspaces
    ? filterMentionWorkspaces(workspaces, session.workspaceId, activeMention.query).slice(0, 12)
    : [];
  const safeMentionIndex = mentionRows.length === 0 ? 0 : Math.min(Math.max(mentionIndex, 0), mentionRows.length - 1);
  usePageOverlay(palette.commands.length > 0 || mentionRows.length > 0 || preview !== null);
  const reading = useRef(false);
  const [showUsage, setShowUsage] = useState(false);
  const [showSavedPrompts, setShowSavedPrompts] = useState(false);
  const [savedPromptsSeed, setSavedPromptsSeed] = useState('');
  const usageShown = useCallback(() => setShowUsage(false), []);
  const paneId = session.id;

  useEffect(() => {
    bridge.call<ComposerState>('composerState', { paneId }).then((value) => { if (value) store.dispatch({ kind: 'composer', state: value }); }, () => {});
  }, [bridge, store, paneId]);

  useLayoutEffect(() => {
    const element = editor.current;
    if (!element) return;
    const line = parseFloat(getComputedStyle(element).lineHeight) || 18;
    element.style.height = 'auto';
    // MC-69: an empty editor is one line, whatever its placeholder says (a long placeholder must not make it taller for a moment).
    if (text.length === 0) return;
    element.style.height = Math.min(element.scrollHeight, line * maxLines + 4) + 'px';
  }, [text]);

  // The palette follows the draft; the host's rows are read when it first opens and again once they are 30 s old.
  const wantsPalette = paletteDraft(session.kind, text, dismissedFor) !== null;
  useEffect(() => {
    if (!wantsPalette || reading.current || (catalog && Date.now() - catalog.read < catalogLifetime)) return;
    reading.current = true;
    bridge.call<SlashCatalog>('slashCommands', { paneId }).then((value) => { if (value) setCatalog({ value, read: Date.now() }); }, () => {}).finally(() => { reading.current = false; });
  }, [bridge, paneId, wantsPalette, catalog]);
  useEffect(() => {
    setPalette((previous) => (catalog ? paletteState(catalog.value, session.kind, text, dismissedFor, previous) : closed));
  }, [catalog, session.kind, text, dismissedFor]);

  const writeDraft = useCallback((draft: string) => {
    void bridge.call('updateDraft', { paneId, text: draft }).catch(() => {});
  }, [bridge, paneId]);
  /** Drops the draft write still waiting: a send sets the host's draft itself, and a late write would put the sent words back. */
  const cancelDraft = useCallback(() => {
    if (draftTimer.current !== null) clearTimeout(draftTimer.current);
    draftTimer.current = null;
    unsaved.current = null;
  }, []);
  const flushDraft = useCallback(() => {
    const draft = unsaved.current;
    cancelDraft();
    if (draft !== null) writeDraft(draft);
  }, [cancelDraft, writeDraft]);
  // A pane closed or switched away writes what was typed.
  useEffect(() => () => flushDraft(), [flushDraft]);
  useEffect(() => () => { if (flashTimer.current !== null) clearTimeout(flashTimer.current); }, []);

  const change = useCallback((value: string) => {
    const draft = value.slice(0, maximumDraft);
    setText(draft);
    edited.current = true;
    unsaved.current = draft;
    if (draftTimer.current !== null) clearTimeout(draftTimer.current);
    draftTimer.current = setTimeout(flushDraft, draftRest);
  }, [flushDraft]);

  const onDictated = useCallback((dictated: string) => {
    const element = editor.current;
    const currentVal = element?.value ?? text;
    const inserted = insertDictatedText(
      currentVal,
      dictated,
      element?.selectionStart,
      element?.selectionEnd,
      maximumDraft,
    );
    change(inserted.text);
    if (element) {
      element.focus();
      requestAnimationFrame(() => element.setSelectionRange(inserted.cursor, inserted.cursor));
    }
  }, [text, change]);

  const voice = useComposerVoice({
    paneId,
    mac: onMac(),
    disabled: !!state?.blocked,
    onDictated,
  });

  const flashQueue = useCallback(() => {
    setFlashing(true);
    if (flashTimer.current !== null) clearTimeout(flashTimer.current);
    flashTimer.current = setTimeout(() => setFlashing(false), Motion.highlight);
  }, []);

  // A card filled the draft (a next action, MainWindow.WebShell.Cards.cs): its words replace the editor's, and it takes the keyboard.
  const filled = useRef(fill?.seq ?? 0);
  useEffect(() => {
    if (!fill || fill.seq === filled.current) return;
    filled.current = fill.seq;
    change(fill.append ? appendingBlock(fill.text, editor.current?.value ?? '') : fill.text);
    const element = editor.current;
    if (element) { element.focus(); requestAnimationFrame(() => element.setSelectionRange(element.value.length, element.value.length)); }
  }, [fill, change]);

  const send = useCallback(async (second: boolean) => {
    if (sending.current) return;
    sending.current = true;
    setError(null);
    const sent = editor.current?.value ?? text;
    const attachments = state?.attachments ?? [];
    // The words go with the send, which sets the host's draft itself: the write still waiting is dropped, then the editor empties.
    cancelDraft();
    setText('');
    edited.current = false;
    walk.current = atDraft;
    // The request is drawn at once when it will start now: not for a queue (the queue row says it), a Ctrl+Enter child (its own
    // pane), a slash command (the host may answer it itself) or a blocked composer.
    const clientId = newClientId();
    const words = sent.trim().length > 0 ? sent.trim() : attachments.map((a) => a.name).join(', ');
    const drawn = !second && state?.primary === 'send' && !state.blocked && words.length > 0 && !sent.trimStart().startsWith('/');
    if (drawn) {
      store.dispatch({ kind: 'pending', action: 'add', paneId, request: newPendingRequest(session, clientId, words, Date.now()) });
    }
    /** Whether the request's block now shows its failure (it may have been replaced by the host's already, or never drawn). */
    const blockShowsFailure = (id: string) => (store.getState().pending[paneId] ?? []).some((request) => request.clientId === id && request.phase === 'failed');
    /** A send that did not go gives its words back, only to an editor still empty and not typed in since. */
    const restore = (draft: string) => {
      if ((editor.current?.value ?? '') !== '' || edited.current || draft.length === 0) return;
      setText(draft);
      writeDraft(draft);
    };
    try {
      const result = await bridge.call<SendResult>('send', { paneId, text: sent, second, attachments: attachments.map((a) => a.id), clientId });
      store.dispatch({ kind: 'composer', state: result.state });
      const message = result.outcome === 'notStarted' ? t('remote.error.startFailed') : result.state.blocked ?? t('composer.blocked.title');
      store.dispatch({ kind: 'pending', action: 'answer', paneId, clientId, outcome: result.outcome, entryId: result.entryId, runId: result.runId, message, at: Date.now() });
      // The host's editor after the send: empty when it went, the words back when it did not (notStarted, blocked, none), or
      // what a built-in command left there.
      restore(result.draft);
      if (result.outcome === 'queued') flashQueue();
      setOutcome(result.outcome);
      // A failure says itself once: in the request's block when one shows it, else in the composer's error row.
      if (result.outcome === 'notStarted' && !blockShowsFailure(clientId)) setError(t('remote.error.startFailed'));
    } catch (failure) {
      const message = failure instanceof Error ? failure.message : String(failure);
      store.dispatch({ kind: 'pending', action: 'fail', paneId, clientId, message, at: Date.now() });
      restore(sent);
      setOutcome('error');
      if (!blockShowsFailure(clientId)) setError(message);
    } finally {
      sending.current = false;
    }
  }, [bridge, store, t, paneId, text, state, session, cancelDraft, writeDraft, flashQueue]);

  const call = useCallback((method: string, args: Record<string, unknown> = {}) => {
    setError(null);
    void bridge.call(method, { paneId, ...args }).catch((failure: unknown) => setError(failure instanceof Error ? failure.message : String(failure)));
  }, [bridge, paneId]);

  const primary = useCallback(() => {
    if (!state) return;
    if (state.primary === 'stop') call('stop');
    else void send(false);
  }, [state, call, send]);

  const pick = async (command: SlashCommand) => {
    const choice = choose(command);
    if (choice.effect !== 'action') {
      setDismissedFor(choice.effect === 'argument' ? null : choice.draft);
      change(choice.draft);
      editor.current?.focus();
      return;
    }
    setDismissedFor(null);
    // The usage popover, saved prompts manager, and issue picker are page panels; every other action is the host's.
    if (choice.action === 'ShowUsage') { change(''); setShowUsage(true); return; }
    if (choice.action === 'ManagePrompts') {
      const current = text;
      change('');
      setSavedPromptsSeed(current);
      setShowSavedPrompts(true);
      return;
    }
    if (choice.action === 'OpenIssues') {
      change('');
      window.dispatchEvent(new CustomEvent('mighty:open-issues', { detail: { workspaceId: session.workspaceId, paneId, settingsOpen: false } }));
      return;
    }
    if (choice.action === 'OpenGitWorkbench') {
      change('');
      window.dispatchEvent(new CustomEvent('mighty:open-git-workbench', { detail: { workspaceId: session.workspaceId, paneId } }));
      return;
    }
    if (choice.action === 'OpenProjectTasks') {
      change('');
      window.dispatchEvent(new CustomEvent('mighty:open-tasks', { detail: { workspaceId: session.workspaceId, paneId } }));
      return;
    }
    if (choice.action === 'OpenAgentContext') {
      change('');
      window.dispatchEvent(new CustomEvent('mighty:open-agent-context', { detail: { workspaceId: session.workspaceId, paneId } }));
      return;
    }
    if (choice.action === 'OpenSharedMemory') {
      change('');
      window.dispatchEvent(new CustomEvent('mighty:open-shared-memory', { detail: { workspaceId: session.workspaceId, paneId } }));
      return;
    }
    try {
      const draft = await bridge.call<string>('slashAction', { paneId, action: choice.action, arg: choice.actionArg });
      const next = draft ?? '';
      setText(next);
      setCaret(next.length);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
    editor.current?.focus();
  };

  const pickMention = (ws: Workspace) => {
    if (!activeMention) return;
    const inserted = applyWorkspaceMention(text, activeMention.start, activeMention.end, ws.name, maximumDraft);
    setDismissedMention(null);
    setMentionIndex(0);
    change(inserted.text);
    setCaret(inserted.cursor);
    const element = editor.current;
    if (element) {
      element.focus();
      requestAnimationFrame(() => element.setSelectionRange(inserted.cursor, inserted.cursor));
    }
  };

  const keyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!event.nativeEvent.isComposing && event.keyCode !== 229 && voice.handleKeyDown(event)) {
      return;
    }
    if (palette.commands.length > 0 && !event.nativeEvent.isComposing && event.keyCode !== 229) {
      const key = paletteKey(event.key, event.shiftKey);
      if (key) {
        event.preventDefault();
        if (key === 'up' || key === 'down') setPalette(move(palette, key === 'up' ? -1 : 1));
        else if (key === 'dismiss') setDismissedFor(text);
        else if (!event.repeat) { const chosen = palette.commands[safeIndex(palette)]; if (chosen) void pick(chosen); }
        return;
      }
    }
    if (mentionRows.length > 0 && !event.nativeEvent.isComposing && event.keyCode !== 229) {
      const key = paletteKey(event.key, event.shiftKey);
      if (key) {
        event.preventDefault();
        if (key === 'up' || key === 'down') {
          const count = mentionRows.length;
          setMentionIndex((safeMentionIndex + (key === 'up' ? -1 : 1) + count) % count);
        } else if (key === 'dismiss') {
          setDismissedMention(mentionKey);
        } else if (!event.repeat) {
          const chosen = mentionRows[safeMentionIndex];
          if (chosen) pickMention(chosen);
        }
        return;
      }
    }
    // MC-106: ↑ on the first line brings the request before, ↓ on the last line the one after, and past the newest the words being typed.
    if ((event.key === 'ArrowUp' || event.key === 'ArrowDown') && !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey
      && !event.nativeEvent.isComposing && event.keyCode !== 229) {
      const element = event.currentTarget;
      const value = element.value;
      const caret = element.selectionStart ?? value.length;
      if (caret === (element.selectionEnd ?? caret)) {
        const history = requestHistory(session.graphRuns);
        const step = event.key === 'ArrowUp'
          ? onFirstLine(value, caret) ? walkUp(walk.current, history, value) : null
          : onLastLine(value, caret) ? walkDown(walk.current, history) : null;
        if (step) {
          event.preventDefault();
          change(step.text);
          walk.current = step.walk;
          requestAnimationFrame(() => element.setSelectionRange(step.text.length, step.text.length));
          return;
        }
      }
    }
    const action = enterAction({ ...event, isComposing: event.nativeEvent.isComposing }, onMac());
    if (action === 'pass') return;
    event.preventDefault();
    if (action === 'swallow') return;
    void send(action === 'second');
  };

  const addFiles = async (files: File[]) => {
    if (files.length === 0 || !state) return;
    if (!state.attachmentsAllowed) return setError(t('wire.startRun.attachmentAiOnly'));
    const refusal = attachmentRefusal(files, state.attachments.length, state.attachments.reduce((sum, a) => sum + a.size, 0));
    if (refusal) return setError(t(refusal));
    try {
      const read = await Promise.all(files.map(async (file) => ({ name: file.name || 'clipboard-image.png', data: toBase64(await file.arrayBuffer()) })));
      await bridge.call('addAttachments', { paneId, files: read });
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : t('composer.attachment.readIncomplete'));
    }
  };

  const paste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = [...event.clipboardData.files];
    if (files.length === 0) return;
    event.preventDefault();
    void addFiles(files);
  };

  const drop = (event: DragEvent<HTMLDivElement>) => {
    setDropping(false);
    if (event.dataTransfer.files.length === 0) return;
    event.preventDefault();
    void addFiles([...event.dataTransfer.files]);
  };

  const openPreview = async (id: string) => setPreview(await bridge.call<AttachmentPreview | null>('attachmentPreview', { paneId, id }));

  const queue = state?.queue;
  const queued = (queue?.held.length ?? 0) + (queue?.items.length ?? 0);
  // 실행 중 보내기: the agent whose new worktree a busy pane's Enter runs the words in (absent: the queue). Its name fills the
  // placeholder the host picks for it (composer.placeholder.busyRunNow) and the primary button's words.
  const agents = state?.worktreeAgents ?? [];
  const runsWith = agents.find((agent) => agent.agent === state?.busySend) ?? null;
  const holdWords = state?.hold ? t('settings.cliUpdate.backgroundUpdateQueued', { provider: state.hold }) : null;
  const placeholder = holdWords ?? t(secondSendWords(state?.placeholderKey ?? (session.kind === 'shell' ? 'composer.placeholder.shell' : 'composer.placeholder.idle'), onMac()), { agent: runsWith?.label ?? '' });
  const primaryName = state?.primary === 'worktree' ? t('queue.runNowAgent', { agent: runsWith?.label ?? '' }) : state?.primary === 'queue' ? t('queue.add') : state?.primary === 'stop' ? t('composer.stop.name') : t('composer.send.name');
  const primaryTipBase = state?.primary === 'worktree' ? t('queue.runNowAgentHint', { agent: runsWith?.label ?? '' }) : state?.primary === 'queue' ? t('queue.addHint') : state?.primary === 'stop' ? t('composer.stop.tooltip') : t('composer.send.tooltip');
  const primaryTip = holdWords ? holdWords + '\n' + primaryTipBase : primaryTipBase;
  // In the busy split control the send button says "send" (↑): the target beside it says where to.
  const inSplit = !!state?.busy && !!state?.pills && (state?.worktreeAgents?.length ?? 0) > 0 && (state?.primary === 'queue' || state?.primary === 'worktree');
  // MC-29: 비교 실행… — an agent pane that is no delegated child offers it on its send menus; the picker takes the editor's words.
  const comparable = session.kind !== 'shell' && !!state?.pills && !session.parentSessionId;
  const openCompare = comparable ? () => window.dispatchEvent(new CustomEvent('mighty:open-compare-picker', { detail: { paneId, text } })) : undefined;
  const [sendMenu, setSendMenu] = useState(false);
  const primaryButton = (
    <button
      type="button"
      className={'composer-primary mode-' + (state?.primary ?? 'send')}
      data-testid="composer-primary"
      aria-label={primaryName}
      title={primaryTip}
      disabled={!state?.primaryEnabled}
      onClick={() => primary()}
      onContextMenu={openCompare && state?.primary === 'send' ? (event) => { event.preventDefault(); setSendMenu(true); } : undefined}
    >
      {state?.primary === 'stop' ? <Icon name="square" size={11} weight={0} fill /> : inSplit ? <Icon name="arrow-up" size={15} weight={2.4} />
        : state?.primary === 'queue' ? <Icon name="list-plus" size={14} weight={2.2} />
        : state?.primary === 'worktree' ? <Icon name="split" size={14} weight={2.2} /> : <Icon name="arrow-up" size={15} weight={2.4} />}
    </button>
  );

  return (
    <div className="composer-region">
      {state?.blocked && (
        <div className="composer-row composer-blocked" data-testid="composer-blocked" role="status">
          <strong>{t('composer.blocked.title')}</strong>
          <span>{state.blocked}</span>
          {state.install && <button type="button" className="composer-text-button" title={t('tooltip.composer.installCli')} onClick={() => call('installCli')}>{t('setup.blocked.install')}</button>}
        </div>
      )}
      {/* MC-102: a CLI updating in the background says so in the editor's placeholder and the send button's tooltip, not in a row
          above the editor that would make it taller for a moment and shake the diagram above. */}
      {state?.hold && <span className="visually-hidden" role="status">{holdWords}</span>}
      {state?.attachmentsLoading && !state.busy && <div className="composer-row" role="status">{t('composer.hint.attachmentsLoading')}</div>}
      {(state?.attachmentError ?? error) && (
        <div className="composer-row composer-error" data-testid="composer-error" role="alert">
          <span>{state?.attachmentError ?? error}</span>
          <button type="button" className="composer-icon-button" title={t('composer.attachment.dismissError')} aria-label={t('composer.attachment.dismissError')} onClick={() => { setError(null); if (state?.attachmentError) call('dismissAttachmentError'); }}>×</button>
        </div>
      )}
      {palette.commands.length > 0 && (
        <div className="slash-palette" data-testid="slash-palette" role="listbox" data-page-overlay="">
          <div className="slash-rows">
            {palette.commands.map((command, index) => (
              <div
                key={command.invocation}
                role="option"
                aria-selected={index === safeIndex(palette)}
                title={'/' + command.invocation + (command.description ? ' — ' + command.description : '')}
                className={'slash-row' + (index === safeIndex(palette) ? ' highlighted' : '')}
                ref={index === safeIndex(palette) ? (row) => row?.scrollIntoView({ block: 'nearest' }) : undefined}
                onMouseEnter={() => setPalette({ ...palette, index })}
                onMouseDown={(event) => { event.preventDefault(); void pick(command); }}
              >
                <span className="slash-name">/{command.invocation}</span>
                <span className="slash-source">{command.source}</span>
                <span className="slash-description">{command.description}</span>
              </div>
            ))}
          </div>
          <div className="slash-count">{t('slash.palette.count', { count: palette.commands.length })}</div>
        </div>
      )}
      {mentionRows.length > 0 && (
        <div className="slash-palette" data-testid="workspace-mention-palette" role="listbox" data-page-overlay="">
          <div className="slash-rows">
            {mentionRows.map((ws, index) => {
              const slug = workspaceMentionSlug(ws.name);
              const folder = workspaceFolderName(ws.path);
              return (
                <div
                  key={ws.id}
                  role="option"
                  aria-selected={index === safeMentionIndex}
                  title={`@workspace:${slug} — ${ws.path}`}
                  className={'slash-row' + (index === safeMentionIndex ? ' highlighted' : '')}
                  ref={index === safeMentionIndex ? (row) => row?.scrollIntoView({ block: 'nearest' }) : undefined}
                  onMouseEnter={() => setMentionIndex(index)}
                  onMouseDown={(event) => { event.preventDefault(); pickMention(ws); }}
                >
                  <span className="slash-name">@workspace:{slug}</span>
                  <span className="slash-source">{ws.name}</span>
                  <span className="slash-description">{t('composer.workspaceMention.hint', { folder })}</span>
                </div>
              );
            })}
          </div>
          <div className="slash-count">{t('composer.workspaceMention.count', { count: mentionRows.length })}</div>
        </div>
      )}
      <div
        className={'composer-card' + (dropping ? ' dropping' : '')}
        data-testid="composer"
        data-last-outcome={outcome ?? undefined}
        onDragOver={(event) => { if (event.dataTransfer.types.includes('Files') && state?.attachmentsAllowed) { event.preventDefault(); setDropping(true); } }}
        onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropping(false); }}
        onDrop={drop}
      >
        {state?.style && <StyleRow paneId={paneId} style={state.style} />}
        {state?.style && <Guided paneId={paneId} panel={state.guided} />}
        {state && state.attachments.length > 0 && (
          <div className="composer-attachments">
            {state.attachments.map((file) => (
              <div className="composer-chip" key={file.id} data-attachment-id={file.id}>
                <button type="button" className="composer-chip-face" title={t('tooltip.composer.attachmentPreview', { name: file.name, size: byteLabel(file.size) })} aria-label={t('composer.attachment.preview', { name: file.name })} onClick={() => void openPreview(file.id)}>
                  <span className="composer-chip-mark">{file.mediaType.startsWith('image/') ? '▣' : '▤'}</span>
                  <span className="composer-chip-words"><span className="composer-chip-name">{file.name}</span><span className="composer-chip-size">{byteLabel(file.size)}</span></span>
                </button>
                <button type="button" className="composer-icon-button" title={t('composer.attachment.remove', { name: file.name })} aria-label={t('composer.attachment.remove', { name: file.name })} onClick={() => call('removeAttachment', { id: file.id })}>×</button>
              </div>
            ))}
          </div>
        )}
        {queue && queued > 0 && (
          <div className={'composer-queue' + (flashing ? ' flash' : '')} data-testid="composer-queue">
            <div className="composer-queue-header">
              <Icon name="rotate-ccw-clock" size={11} weight={2.4} className="composer-queue-clock" />
              <span>{t(state.busy ? 'queue.waitingBusy' : 'queue.waiting', { count: queued })}</span>
            </div>
            {queue.waitsOnBackground && <div className="composer-queue-notice">{t(secondSendWords(queue.waitsOnBackground, onMac()))}</div>}
            {queue.held.map((item, index) => {
              // A child's notice reads as a chip (its title and what happened); the pointer text the agent gets is its tooltip.
              const notice = parseDelegationNotice(item.text);
              return (
              <div className="composer-queue-item" key={item.id} data-queue-held={item.id}>
                <span className="composer-queue-number">{index + 1}</span>
                <span className="composer-queue-text">{notice ? <NoticeChip notice={notice} /> : item.text}</span>
                <RunNowButton paneId={paneId} itemId={item.id} runNow={readRunNow(item.runNow)} held={queue.held.length} agents={agents} own={session.provider} />
                <span className="composer-queue-pin" title={t('queue.heldHelp')} aria-label={t('queue.held')}><Icon name="pin" size={10} weight={2.2} fill /></span>
              </div>
              );
            })}
            {queue.items.map((item, index) => (
              // MC-49: a mirrored pane's rows are its host's (queue.remote): their buttons act there.
              <QueueRow key={item.id} paneId={paneId} item={item} index={index} count={queue.items.length} held={queue.held.length}
                buttons={queueRowButtons(readRunNow(item.runNow), queue.remote, item.locked)} remote={!!queue.remote} agents={agents}
                own={session.provider} call={call} />
            ))}
          </div>
        )}
        <VoiceStatusBar
          phase={voice.phase}
          levels={voice.levels}
          partial={voice.partial}
          seconds={voice.seconds}
          permissionDenied={voice.permissionDenied}
          voiceError={voice.voiceError}
          onStop={() => voice.stopVoice(false)}
          onCancel={() => voice.stopVoice(true)}
          onOpenSettings={voice.openPermissionSettings}
          onDismissPermission={voice.dismissPermission}
          onDismissError={voice.dismissError}
        />
        <textarea
          ref={editor}
          className="composer-input"
          data-testid="composer-input"
          rows={1}
          value={text}
          maxLength={maximumDraft}
          placeholder={placeholder}
          aria-label={t('composer.input.name')}
          title={t('composer.inputShortcuts')}
          onChange={(event) => { walk.current = atDraft; setCaret(event.target.selectionStart ?? event.target.value.length); change(event.target.value); }}
          onSelect={(event) => setCaret(event.currentTarget.selectionStart ?? 0)}
          onKeyDown={keyDown}
          onPaste={paste}
        />
        <ComposerCard paneId={paneId} kind="login" />
        <div className="composer-toolbar">
          {state?.attachmentsAllowed !== false && (
            <button type="button" className="composer-pill compact composer-attach" aria-label={t('composer.attach.name')} title={t('tooltip.composer.attach')} disabled={state?.attachmentsLoading} onClick={() => call('pickAttachments')}><Icon name="paperclip" size={14} weight={1.75} /></button>
          )}
          {session.kind !== 'shell' && (
            <button
              type="button"
              className="composer-pill compact"
              data-testid="composer-open-issues"
              aria-label={t('integrations.open')}
              title={t('integrations.open')}
              onClick={() => window.dispatchEvent(new CustomEvent('mighty:open-issues', { detail: { workspaceId: session.workspaceId, paneId, settingsOpen: false } }))}
            >
              <Icon name="list-todo" size={14} weight={1.75} />
            </button>
          )}
          {state?.pills ? <Toolbar paneId={paneId} pills={state.pills} busy={state.busy} showUsage={showUsage} onUsageShown={usageShown} resumed={state.resumed} worktreeAgents={agents} busySend={runsWith?.agent ?? null} />
            : <span className="composer-shell-title">{t('composer.shell.title')}</span>}
          {state?.toggles && state.toggles.statusLine !== null && (
            <button type="button" className={'composer-toggle' + (state.toggles.statusLine ? ' on' : '')} aria-pressed={state.toggles.statusLine} title={t(state.toggles.statusLine ? 'composer.statusLine.hide' : 'composer.statusLine.show')}
              aria-label={t('composer.statusLine.name')} onClick={() => call('composerToggle', { which: 'statusLine' })}><Icon name="panel-bottom" size={13} weight={1.75} /></button>
          )}
          {(state?.primary === 'queue' || state?.primary === 'worktree') && (
            <button type="button" className="composer-stop-small" aria-label={t('composer.stop.name')} title={t('composer.stop.tooltip')} disabled={state.stopping} onClick={() => call('stop')}><Icon name="square" size={9} weight={0} fill /></button>
          )}
          <VoiceMicButton
            phase={voice.phase}
            mac={onMac()}
            disabled={!!state?.blocked}
            onPointerDown={voice.onMicPointerDown}
            onPointerUp={voice.onMicPointerUp}
          />
          {/* 실행 중 보내기: while the pane runs and words wait to go, Enter's target (대기열, or an agent's worktree) is chosen on the
              send button itself, one split control [target ▾ | ↑]; the separate worktree disc is there only while Enter queues, since in
              worktree mode the send button already does what it would. */}
          {state?.worktree && !(state.busy && state.pills && state.primary === 'worktree') && (
            <button type="button" className="composer-worktree" data-testid="composer-worktree" aria-label={t('queue.worktree')} title={t(onMac() ? 'queue.worktreeHintMac' : 'queue.worktreeHint')} disabled={!state.worktreeEnabled} onClick={() => void send(true)}><Icon name="split" size={13} weight={2.2} /></button>
          )}
          {state?.busy && state.pills && agents.length > 0 && (state.primary === 'queue' || state.primary === 'worktree') ? (
            <span className="composer-send-split" data-testid="composer-send-split">
              <BusySendPill paneId={paneId} agents={agents} current={runsWith?.agent ?? null} split onCompare={openCompare} />
              {primaryButton}
            </span>
          ) : (
            <>
              {state?.busy && state.pills && <BusySendPill paneId={paneId} agents={agents} current={runsWith?.agent ?? null} compact onCompare={openCompare} />}
              {primaryButton}
            </>
          )}
          {sendMenu && openCompare && <SendMenu onCompare={openCompare} onClose={() => setSendMenu(false)} />}
        </div>
        <ComposerCard paneId={paneId} kind="statusLine" />
      </div>
      {showSavedPrompts && (
        <SavedPromptsPanel
          open={showSavedPrompts}
          paneId={paneId}
          workspaceId={session.workspaceId || null}
          initialDraft={savedPromptsSeed}
          onClose={() => setShowSavedPrompts(false)}
          onChanged={() => setCatalog(null)}
          onPutInComposer={(expanded, sendNow) => {
            if (sendNow) {
              store.dispatch({ kind: 'fill', paneId, text: expanded, append: false });
              setTimeout(() => {
                void bridge.call('send', { paneId, text: expanded, second: false }).then(() => {
                  change('');
                }, () => {});
              }, 0);
            } else {
              const merged = text.trim().length > 0 ? `${text.trimEnd()}\n\n${expanded}` : expanded;
              change(merged);
              setCaret(merged.length);
              editor.current?.focus();
            }
          }}
        />
      )}
      {preview && (
        <div className="composer-preview" role="dialog" data-page-overlay="" aria-label={preview.name} onClick={() => setPreview(null)}>
          <div className="composer-preview-card" onClick={(event) => event.stopPropagation()}>
            <div className="composer-preview-title">{preview.name}</div>
            <div className="composer-preview-meta">{preview.mediaType} · {preview.size.toLocaleString()} bytes</div>
            {preview.image ? <img src={preview.image} alt={preview.name} /> : preview.text !== undefined ? <pre>{preview.text}</pre> : <p>{t('composer.attachment.sentAsFile')}</p>}
            <button type="button" className="composer-text-button" title={t('tooltip.composer.closePreview')} onClick={() => setPreview(null)}>{t('settings.closeButton')}</button>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * One queued row (MC-49): its number and words, 바로 실행, 고치기, ↑/↓ and ×, as `buttons` allows. 고치기 turns the words into an editor
 * in the row (Enter saves, Shift+Enter a new line, Esc cancels; words none are refused and the editor stays). ↑/↓ on the focused row
 * move it; the row keeps the focus at its new place. The rule for each is the host's: the page only asks.
 */
function QueueRow({ paneId, item, index, count, held, buttons, remote, agents, own, call }: {
  paneId: string; item: QueueItem; index: number; count: number; held: number; buttons: QueueRowButtons; remote: boolean;
  agents: readonly WorktreeAgent[]; own: string; call: (method: string, args?: Record<string, unknown>) => void;
}) {
  const { bridge, t } = useServices();
  const [editing, setEditing] = useState<string | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const row = useRef<HTMLDivElement | null>(null);
  const moved = useRef(false);
  // A row moved from the keyboard keeps the focus at its new place (the page draws the new order when the host pushes it).
  useEffect(() => {
    if (moved.current) {
      moved.current = false;
      row.current?.focus();
    }
  }, [index]);
  const moveTo = (to: number | null, keyboard = false) => {
    if (to === null) return;
    moved.current = keyboard;
    call('queueMove', { itemId: item.id, to });
  };
  const save = async () => {
    if (editing === null || saving) return;
    const words = editedWords(editing, item.attachments.length);
    if (words === null) { setRefusal(t('queue.editEmpty')); return; }
    if (words === item.text) { setEditing(null); setRefusal(null); return; }
    setSaving(true);
    try {
      await bridge.call('queueEdit', { paneId, itemId: item.id, text: words });
      setEditing(null);
      setRefusal(null);
    } catch (failure) {
      setRefusal(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setSaving(false);
    }
  };
  const cancel = () => { setEditing(null); setRefusal(null); row.current?.focus(); };
  return (
    <div className="composer-queue-item" ref={row} data-queue-item={item.id} data-queue-remote={remote ? '' : undefined}
      tabIndex={buttons.move ? 0 : undefined} title={buttons.move && editing === null ? t('queue.rowHelp') : undefined}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget || !buttons.move) return;
        const to = queueMoveTarget(index, count, event.key);
        if (to === null) return;
        event.preventDefault();
        moveTo(to, true);
      }}>
      <span className="composer-queue-number">{held + index + 1}</span>
      {editing !== null ? (
        <span className="composer-queue-editing">
          <textarea className="composer-queue-editor" value={editing} rows={Math.min(6, Math.max(1, editing.split('\n').length))} autoFocus
            aria-label={t('queue.edit')} aria-invalid={refusal !== null || undefined} disabled={saving}
            onChange={(event) => { setEditing(event.target.value); setRefusal(null); }}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing || event.keyCode === 229) return;
              if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); cancel(); }
              else if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); void save(); }
            }} />
          {refusal && <span className="composer-queue-refusal" role="alert">{refusal}</span>}
        </span>
      ) : (
        <span className="composer-queue-text">
          {item.text.length === 0 ? t('queue.attachmentsOnly') : item.text}
          {item.attachments.length > 0 && <span className="composer-queue-files">{item.attachments.join(', ')}</span>}
        </span>
      )}
      {editing !== null ? (
        <>
          <button type="button" className="composer-icon-button" title={t('queue.editSave')} aria-label={t('queue.editSave')} disabled={saving} onClick={() => void save()}><Icon name="check" size={12} weight={2.2} /></button>
          <button type="button" className="composer-icon-button" title={t('common.cancel')} aria-label={t('common.cancel')} onClick={cancel}>×</button>
        </>
      ) : (
        <>
          <RunNowButton paneId={paneId} itemId={item.id} runNow={buttons.runNow} held={held} agents={agents} own={own} remote={!buttons.pick} />
          {buttons.edit && <button type="button" className="composer-icon-button" title={t('queue.editHelp')} aria-label={t('queue.edit')} onClick={() => { setEditing(item.text); setRefusal(null); }}><Icon name="pencil" size={11} weight={2.2} /></button>}
          {buttons.move && <button type="button" className="composer-icon-button" title={t('queue.moveUp')} aria-label={t('queue.moveUp')} aria-disabled={index === 0 || undefined} onClick={() => moveTo(queueMoveTarget(index, count, 'ArrowUp'))}><Icon name="arrow-up" size={11} weight={2.2} /></button>}
          {buttons.move && <button type="button" className="composer-icon-button" title={t('queue.moveDown')} aria-label={t('queue.moveDown')} aria-disabled={index === count - 1 || undefined} onClick={() => moveTo(queueMoveTarget(index, count, 'ArrowDown'))}><Icon name="arrow-down" size={11} weight={2.2} /></button>}
          {buttons.remove && <button type="button" className="composer-icon-button" title={t('tooltip.composer.queueRemove')} aria-label={t('queue.remove')} onClick={() => call('queueRemove', { itemId: item.id })}>×</button>}
        </>
      )}
    </div>
  );
}

/**
 * 바로 실행 beside a queued row's × or the first held row's pin (M/QueuedInputsView.swift runNowButton): what the host decided
 * for the row (Core QueueRunNow). A worktree row has the agent pick before it: the worktree menu's agents, the pane's own agent
 * first chosen ([`rowAgent`]); 바로 실행 runs the row in a new worktree of the agent picked. A row that must wait is drawn with
 * aria-disabled, not disabled: Chromium shows no title on a disabled button, and the reason must stay readable and the button
 * focusable. A null runNow draws nothing.
 */
function RunNowButton({ paneId, itemId, runNow, held, agents, own, remote = false }: { paneId: string; itemId: string; runNow: RunNow | null; held: number; agents: readonly WorktreeAgent[]; own: string; remote?: boolean }) {
  const { bridge, t } = useServices();
  const [picked, setPicked] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const menuRef = usePopover(close);
  usePageOverlay(open);
  if (runNow === null) return null;
  const branch = runNowBranches(runNow);
  // A host's row (MC-49) runs there with the host pane's own agent: no pick here.
  const agent = branch && !remote ? rowAgent(agents, picked, own) : null;
  // A worktree row with no agent that can run now waits, as the host would refuse it. (The host sends the worktree menu's agents
  // whenever it offers a worktree, which a worktree row needs.)
  const noAgent = branch && !remote && agent === null;
  const waits = remote ? remoteRunNowWaits(runNow) : runNowWaits(runNow) || noAgent;
  const run = () => { void bridge.call('queueRunNow', { paneId, itemId, ...(agent ? { agent: agent.agent } : {}) }).catch(() => {}); };
  const [key, values] = remote ? remoteRunNowTip(runNow, held) : noAgent ? ['queue.runNowUnavailableHelp'] : runNowTip(runNow, held, agent?.label ?? '');
  return (
    <span className="composer-queue-run-now" ref={menuRef}>
      {branch && !remote && agents.length > 0 && (
        <button type="button" className="composer-text-button composer-queue-agent" data-run-now-agent={itemId} aria-haspopup="menu" aria-expanded={open}
          aria-label={t('queue.runNowAgentPick', { agent: agent?.label ?? '' })} title={t('queue.runNowAgentPick', { agent: agent?.label ?? '' })} onClick={() => setOpen(!open)}>
          {agent && <ProviderMark provider={agent.agent} size={11} />}<Icon name="chevron-down" size={7} weight={3} />
        </button>
      )}
      <button type="button" className={'composer-text-button composer-queue-run' + (waits ? ' waits' : '')} data-run-now={itemId} title={t(key, values)}
        aria-disabled={waits || undefined} onClick={() => { if (!waits) run(); }}>
        <Icon name={branch ? 'split' : 'play'} size={9} weight={branch ? 2.2 : 1} fill={!branch} />{t('queue.runNow')}
      </button>
      {open && (
        <div className="composer-popover popover composer-menu" role="menu" data-menu="run-now" data-page-overlay="">
          <MenuList rows={[{ kind: 'header', text: t('composer.worktreeMenu.title') }, ...agents.map((a) => ({ kind: 'choice' as const, text: a.detail, enabled: a.enabled, checked: a.agent === agent?.agent }))]}
            path={[]} choose={(path) => { const choice = agents[(path[0] ?? 0) - 1]; setOpen(false); if (choice) setPicked(choice.agent); }} />
        </div>
      )}
    </span>
  );
}

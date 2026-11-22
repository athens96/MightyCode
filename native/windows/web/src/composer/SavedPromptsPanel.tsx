// Saved requests panel (/prompts): two-column panel matching macOS SavedPromptsSheet.swift.
// Left sidebar lists workspace and shared prompts; right editor creates, edits, deletes, inserts, or sends a prompt.
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import './SavedPromptsPanel.css';

export interface SavedPromptItem {
  id: string;
  name: string;
  text: string;
  workspaceId?: string | null;
}

export interface SavedPromptsPanelProps {
  open: boolean;
  paneId: string | null;
  workspaceId: string | null;
  initialDraft?: string;
  onClose: () => void;
  onPutInComposer: (text: string, sendNow: boolean) => void;
  onChanged: () => void;
}

interface SavedPromptsListResponse {
  prompts: SavedPromptItem[];
  offered: SavedPromptItem[];
}

const VALID_PROMPT_CHAR = /[\p{L}\p{N}_-]/u;
const MAX_NAME_CODE_POINTS = 40;

/**
 * Normalizes a raw prompt name into a slash-invokable identifier:
 * lowercases, trims surrounding whitespace, turns spaces into hyphens,
 * keeps Unicode letters, digits, `-` and `_`, caps at 40 code points,
 * and returns `null` when nothing valid remains.
 */
export function normalizeSavedPromptName(raw: string): string | null {
  const mapped = Array.from(raw.trim().toLowerCase()).map((ch) => (ch === ' ' ? '-' : ch));
  const kept = mapped
    .filter((ch) => VALID_PROMPT_CHAR.test(ch))
    .slice(0, MAX_NAME_CODE_POINTS)
    .join('');
  return kept.length > 0 ? kept : null;
}

/** Initial textarea value when starting a new saved prompt from the composer's draft. */
export function initialPromptDraftText(initialDraft?: string): string {
  const trimmed = (initialDraft ?? '').trim();
  if (trimmed.startsWith('/prompts')) return '';
  return trimmed;
}

function renderInBodyPortal(node: ReactNode) {
  if (typeof document !== 'undefined' && document.body) {
    return createPortal(node, document.body);
  }
  return node;
}

export function SavedPromptsPanel(props: SavedPromptsPanelProps) {
  const { open, paneId, workspaceId, initialDraft, onClose, onPutInComposer, onChanged } = props;
  const { bridge, t } = useServices();
  usePageOverlay(open);

  const [offered, setOffered] = useState<SavedPromptItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [text, setText] = useState(() => (open ? initialPromptDraftText(initialDraft) : ''));
  const [shareAll, setShareAll] = useState(true);
  const wasOpenRef = useRef(open);
  const latestDraftRef = useRef(initialDraft);
  latestDraftRef.current = initialDraft;

  const effectiveShareAll = workspaceId === null ? true : shareAll;
  const normalized = useMemo(() => normalizeSavedPromptName(name), [name]);
  const canSave = Boolean(normalized && text.trim().length > 0);
  const canUse = text.trim().length > 0;

  const startNew = useCallback((fromDraft: boolean) => {
    setSelectedId(null);
    setName('');
    setText(fromDraft ? initialPromptDraftText(latestDraftRef.current) : '');
    setShareAll(true);
    setError(null);
  }, []);

  const selectPrompt = useCallback((prompt: SavedPromptItem) => {
    setSelectedId(prompt.id);
    setName(prompt.name);
    setText(prompt.text);
    setShareAll(prompt.workspaceId == null);
    setError(null);
  }, []);

  const refreshList = useCallback(async () => {
    const res = await bridge.call<SavedPromptsListResponse>('savedPromptsList', { workspaceId });
    const list = res?.offered ?? res?.prompts ?? [];
    setOffered(list);
    return list;
  }, [bridge, workspaceId]);

  useEffect(() => {
    if (!open) {
      wasOpenRef.current = false;
      return undefined;
    }
    if (!wasOpenRef.current) {
      wasOpenRef.current = true;
      startNew(true);
    }
    let live = true;
    setLoading(true);
    void bridge
      .call<SavedPromptsListResponse>('savedPromptsList', { workspaceId })
      .then(
        (res) => {
          if (!live) return;
          setOffered(res?.offered ?? res?.prompts ?? []);
          setLoading(false);
        },
        (reason: unknown) => {
          if (!live) return;
          setOffered([]);
          setLoading(false);
          setError(reason instanceof Error ? reason.message : String(reason));
        },
      );
    return () => {
      live = false;
    };
  }, [open, workspaceId, bridge, startNew]);

  if (!open) return null;

  const handleSave = async () => {
    if (!normalized || text.trim().length === 0 || busy) return;
    setBusy(true);
    setError(null);
    try {
      const targetWorkspaceId = effectiveShareAll ? null : workspaceId;
      const saved = await bridge.call<{ prompt: SavedPromptItem }>('savedPromptsSave', {
        id: selectedId,
        name: normalized,
        text,
        workspaceId: targetWorkspaceId,
      });
      const list = await refreshList();
      const chosen =
        saved?.prompt ??
        list.find((item) => item.name === normalized && (item.workspaceId ?? null) === targetWorkspaceId) ??
        null;
      if (chosen) {
        selectPrompt(chosen);
      }
      onChanged();
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleDelete = async () => {
    if (!selectedId || busy) return;
    setBusy(true);
    setError(null);
    try {
      await bridge.call('savedPromptsDelete', { id: selectedId });
      await refreshList();
      startNew(false);
      onChanged();
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleExpandAndUse = async (sendNow: boolean) => {
    if (!canUse || busy) return;
    setBusy(true);
    setError(null);
    try {
      const selectedItem = selectedId ? offered.find((item) => item.id === selectedId) : undefined;
      const effectivePromptId = selectedItem && selectedItem.text === text ? selectedId : null;
      const res = await bridge.call<{ text: string }>('savedPromptsExpand', {
        paneId,
        promptId: effectivePromptId,
        text,
      });
      onPutInComposer(res?.text ?? text, sendNow);
      onClose();
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const dialog = (
    <DialogShell
      kind="panel"
      panel="savedPrompts"
      width={740}
      height={480}
      minWidth={540}
      minHeight={360}
      label={t('savedPrompts.title')}
      title={t('savedPrompts.title')}
      subtitle={t('savedPrompts.subtitle')}
      icon={<Icon name="sparkles" size={18} weight={1.8} />}
      busy={busy}
      onCancel={onClose}
      testId="saved-prompts-panel"
      closeTestId="saved-prompts-close"
      bodyClassName="saved-prompts-body"
      backdrop={{ 'data-page-overlay': '' }}
      status={error ? <span className="tone-text-err">{error}</span> : undefined}
    >
      <div className="saved-prompts-columns">
        <aside className="saved-prompts-sidebar">
          <button
            type="button"
            className="saved-prompts-new-btn"
            data-testid="saved-prompts-new"
            disabled={busy}
            onClick={() => startNew(false)}
          >
            <Icon name="plus" size={13} weight={2} />
            <span>{t('savedPrompts.new')}</span>
          </button>

          <div className="saved-prompts-list" role="listbox" aria-label={t('savedPrompts.title')}>
            {loading ? (
              <DialogLoading rows={3} />
            ) : offered.length === 0 ? (
              <div className="saved-prompts-empty" data-testid="saved-prompts-empty">
                {t('savedPrompts.empty')}
              </div>
            ) : (
              offered.map((prompt) => {
                const isSelected = selectedId === prompt.id;
                const scopeLabel = prompt.workspaceId
                  ? t('savedPrompts.scope.workspace')
                  : t('savedPrompts.scope.all');
                return (
                  <button
                    key={prompt.id}
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    className={'saved-prompts-item' + (isSelected ? ' selected' : '')}
                    data-testid={'saved-prompts-item-' + prompt.name}
                    onClick={() => selectPrompt(prompt)}
                  >
                    <span className="saved-prompts-item-cmd mono">{'/prompt:' + prompt.name}</span>
                    <span className="saved-prompts-item-scope">{scopeLabel}</span>
                  </button>
                );
              })
            )}
          </div>
        </aside>

        <section className="saved-prompts-editor">
          <div className="saved-prompts-name-block">
            <input
              type="text"
              className="dialog-field saved-prompts-name-input mono"
              data-autofocus
              data-testid="saved-prompts-name"
              placeholder={t('savedPrompts.name')}
              aria-label={t('savedPrompts.name')}
              value={name}
              disabled={busy}
              onChange={(event) => setName(event.target.value)}
            />
            <div
              className={
                'saved-prompts-preview' + (normalized === null && name.length > 0 ? ' invalid' : '')
              }
              data-testid="saved-prompts-preview"
            >
              {normalized !== null
                ? t('savedPrompts.invokeAs', { command: '/prompt:' + normalized })
                : name.length > 0
                  ? t('savedPrompts.nameRule')
                  : '\u00A0'}
            </div>
          </div>

          <textarea
            className="saved-prompts-textarea"
            data-testid="saved-prompts-text"
            placeholder={t('savedPrompts.textPlaceholder')}
            aria-label={t('savedPrompts.textPlaceholder')}
            value={text}
            disabled={busy}
            onChange={(event) => setText(event.target.value)}
          />

          <div className="saved-prompts-hint">{t('savedPrompts.placeholders')}</div>

          <label className="saved-prompts-share">
            <input
              type="checkbox"
              data-testid="saved-prompts-share-all"
              checked={effectiveShareAll}
              disabled={workspaceId === null || busy}
              onChange={(event) => setShareAll(event.target.checked)}
            />
            <span>{t('savedPrompts.shareAll')}</span>
          </label>

          <div className="saved-prompts-actions">
            {selectedId !== null && (
              <button
                type="button"
                className="btn danger"
                data-testid="saved-prompts-delete"
                disabled={busy}
                onClick={() => {
                  void handleDelete();
                }}
              >
                {t('savedPrompts.delete')}
              </button>
            )}
            <div className="saved-prompts-actions-spacer" />
            <button
              type="button"
              className="btn"
              data-testid="saved-prompts-use"
              disabled={!canUse || busy}
              onClick={() => {
                void handleExpandAndUse(false);
              }}
            >
              {t('savedPrompts.use')}
            </button>
            <button
              type="button"
              className="btn"
              data-testid="saved-prompts-send-now"
              disabled={!canUse || busy}
              onClick={() => {
                void handleExpandAndUse(true);
              }}
            >
              {t('savedPrompts.sendNow')}
            </button>
            <button
              type="button"
              className="btn primary"
              data-testid="saved-prompts-save"
              disabled={!canSave || busy}
              onClick={() => {
                void handleSave();
              }}
            >
              {t('savedPrompts.save')}
            </button>
          </div>
        </section>
      </div>
    </DialogShell>
  );

  return renderInBodyPortal(dialog);
}

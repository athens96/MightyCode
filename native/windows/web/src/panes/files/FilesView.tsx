import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent as ReactMouseEvent } from 'react';
import type { RunSession } from '../../store/models';
import { useServices } from '../../app/context';
import { Markdown } from '../../conversation/MarkdownView';
import { useReferenceClick, useReferenceKey } from '../../conversation/referenceClick';
import { MiddlePath, useMenu } from '../../shell/parts';
import { Icon, type IconName } from '../../icons/Icon';
import { HtmlFrame, isHtmlPreview } from './HtmlView';
import { DiffView } from './DiffView';
import { CodeEditor, FindReplaceBar } from './CodeEditor';
import {
  computeBufferChanges,
  findMatches,
  formatAgentSelectionBlock,
  formatSelectionReference,
  pickTargetAgentSession,
  replaceAllMatches,
  replaceSingleMatch,
  resolveDomCodeSelection,
  type CodeSelection,
} from './editorModel';
import { appendingBlock } from '../../mighty/runChangesModel';
import { File as PierreFile } from '@pierre/diffs/react';
import {
  DEFAULT_FILE_ROW_HEIGHT,
  DEFAULT_TREE_OVERSCAN,
  TREE_WINDOW_THRESHOLD,
  fileRowId,
  scrollRowIntoView,
  treeKey,
  visibleTreeSlice,
} from './tree';
import { useAppTheme } from './useAppTheme';
import { listenPane } from '../channel';
import type { FileChanges, FilePreview, FileRow, FilesDiskCheckResult, FilesModel, FilesSaveResult } from './types';

export { fileRowId, visibleTreeSlice };

type TextViewMode = 'rendered' | 'source' | 'edit' | 'diff';

/** The symbols of the tree and the head (M/FilePaneView.swift, WinUI FileSymbol): folder, page, picture, source as </>. */
function Symbol({ row, symbol }: { row?: FileRow; symbol?: string }) {
  const kind = row?.dir ? 'folder' : symbol ?? row?.symbol ?? 'unsupported';
  const name: IconName = kind === 'folder' ? 'folder' : kind === 'image' ? 'image' : kind === 'markdown' ? 'file-text' : kind === 'source' ? 'code-xml' : 'file';
  return <span className={'files-symbol files-symbol-' + kind} aria-hidden="true"><Icon name={name} size={13} weight={1.6} /></span>;
}

export const Tree = memo(function Tree({ model, dirtyPaths, onRow, onKey, onMenu }: {
  model: FilesModel;
  dirtyPaths?: ReadonlySet<string>;
  onRow: (row: FileRow) => void;
  onKey: (event: KeyboardEvent<HTMLDivElement>) => void;
  /** A right-click on a row: its menu (편집기에서 열기 and the desktop's actions). */
  onMenu?: (event: ReactMouseEvent, row: FileRow) => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  const windowed = model.rows.length > TREE_WINDOW_THRESHOLD;
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(600);
  const [rowHeight, setRowHeight] = useState(DEFAULT_FILE_ROW_HEIGHT);

  useLayoutEffect(() => {
    const element = list.current;
    if (!windowed || !element) return undefined;
    const measure = () => {
      setViewportHeight((current) => (element.clientHeight > 0 && element.clientHeight !== current ? element.clientHeight : current));
      const firstRow = element.querySelector<HTMLElement>('.files-row-group');
      const measured = firstRow?.offsetHeight ?? 0;
      if (measured > 0) setRowHeight((current) => (current === measured ? current : measured));
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [windowed, model.rows.length]);

  useEffect(() => {
    const element = list.current;
    if (!element || !model.selected) return;
    const rendered = element.querySelector<HTMLElement>('[aria-selected="true"]');
    if (rendered) {
      rendered.scrollIntoView({ block: 'nearest' });
      return;
    }
    if (!windowed) return;
    const index = model.rows.findIndex((candidate) => candidate.path === model.selected);
    const nextScroll = scrollRowIntoView(index, rowHeight, element.scrollTop, element.clientHeight || viewportHeight);
    if (nextScroll !== null) {
      element.scrollTop = nextScroll;
      setScrollTop(nextScroll);
    }
  }, [model.selected, model.rows, windowed, rowHeight, viewportHeight]);

  const slice = windowed ? visibleTreeSlice(model.rows.length, scrollTop, viewportHeight, rowHeight, DEFAULT_TREE_OVERSCAN) : null;
  const shownRows = slice ? model.rows.slice(slice.start, slice.end) : model.rows;

  return (
    <div className="files-tree" role="tree" tabIndex={0} ref={list} data-testid="files-tree"
      aria-activedescendant={model.selected ? fileRowId(model.selected) : undefined}
      onKeyDown={onKey}
      onScroll={windowed ? (event) => setScrollTop(event.currentTarget.scrollTop) : undefined}>
      {slice && slice.topSpacer > 0 && <div aria-hidden="true" style={{ height: slice.topSpacer }} />}
      {shownRows.map((row) => (
        <div key={row.path} className="files-row-group">
          <div id={fileRowId(row.path)} role="treeitem" className={'files-row' + (row.noise ? ' noise' : '')} aria-selected={row.path === model.selected} aria-expanded={row.dir && !model.filtering ? row.expanded : undefined}
            data-path={row.path} title={row.path} style={{ paddingLeft: 'calc(var(--space-xs) + ' + (8 + row.depth * 14) + 'px)' }} onClick={() => onRow(row)}
            onContextMenu={onMenu ? (event) => onMenu(event, row) : undefined}>
            <span className="files-chevron">{row.dir && !model.filtering && <Icon name={row.expanded ? 'chevron-down' : 'chevron-right'} size={10} weight={2.8} />}</span>
            <Symbol row={row} />
            <span className="files-name">{row.name}</span>
            {row.symlink && <span className="files-link" aria-hidden="true">↗</span>}
            {model.filtering && row.path !== row.name && <MiddlePath className="files-row-path" path={row.path} />}
            {!row.dir && dirtyPaths?.has(row.path) && <span className="files-row-dirty" aria-hidden="true" />}
          </div>
          {row.caption && !model.filtering && (
            <div className={'files-caption' + (row.reports ? ' reports' : '')} style={{ paddingLeft: 8 + (row.depth + 1) * 14 + 29 + 'px' }}>{row.caption}</div>
          )}
        </div>
      ))}
      {slice && slice.bottomSpacer > 0 && <div aria-hidden="true" style={{ height: slice.bottomSpacer }} />}
      {model.notes.map((note) => <div key={note} className="files-note">{note}</div>)}
    </div>
  );
});

/**
 * A source file as text with its line numbers, coloured by @pierre/diffs (Shiki's grammars, the same as VS Code's and as the
 * 변경사항 view's), its language from the file's name; a long line wraps when the host says so.
 */
export function SourceView({
  preview,
  onSelectionChange,
  onContextMenuSelection,
}: {
  preview: FilePreview;
  onSelectionChange?: (selection: CodeSelection | null) => void;
  onContextMenuSelection?: (event: ReactMouseEvent, selection: CodeSelection | null) => void;
}) {
  const theme = useAppTheme();
  const containerRef = useRef<HTMLDivElement>(null);
  const contents = preview.text ?? '';
  const file = useMemo(() => ({ name: preview.name, contents }), [preview.name, contents]);
  const options = useMemo(() => ({
    theme: { dark: 'pierre-dark', light: 'pierre-light' },
    themeType: theme,
    overflow: preview.wraps ? ('wrap' as const) : ('scroll' as const),
    disableFileHeader: true,
  }), [theme, preview.wraps]);

  const checkDomSelection = useCallback(() => {
    if (!onSelectionChange) return;
    onSelectionChange(resolveDomCodeSelection(containerRef.current, contents));
  }, [contents, onSelectionChange]);

  return (
    <div
      ref={containerRef}
      className="files-source highlighted"
      data-testid="files-source"
      onMouseUp={checkDomSelection}
      onKeyUp={checkDomSelection}
      onContextMenu={onContextMenuSelection ? (event) => {
        const sel = resolveDomCodeSelection(containerRef.current, contents);
        onSelectionChange?.(sel);
        onContextMenuSelection(event, sel);
      } : undefined}
    >
      <PierreFile file={file} options={options} />
    </div>
  );
}

/** Fit (never larger than 1:1), zoom out, 1:1 and zoom in (M/FilePaneView.swift:202-216, 327-352); a CSS zoom of the host's picture. */
export function ImageView({ preview, zoom }: { preview: FilePreview; zoom: number | null }) {
  const image = preview.image;
  if (!image) return null;
  const ratio = image.bitmap ? window.devicePixelRatio || 1 : 1;
  const width = image.width / ratio, height = image.height / ratio;
  const style = zoom === null ? { maxWidth: '100%', maxHeight: '100%', width, height: 'auto' } : { width: width * zoom, height: height * zoom, maxWidth: 'none' };
  return (
    <div className={'files-image' + (zoom === null ? ' fit' : '')} data-testid="files-image">
      <img src={image.url} alt={preview.name} style={style} draggable={false} />
    </div>
  );
}

function Unsupported({ preview, reveal }: { preview: FilePreview; reveal: () => void }) {
  const { t } = useServices();
  return (
    <div className="files-unsupported" data-testid="files-unsupported">
      <h2>{t('files.preview.unsupported')}</h2>
      <p className="files-unsupported-name">{preview.name}</p>
      <dl>
        <dt>{t('files.preview.size')}</dt><dd>{preview.size}</dd>
        {preview.modified && <><dt>{t('files.preview.modified')}</dt><dd>{preview.modified}</dd></>}
      </dl>
      {preview.reason && <p className="files-unsupported-reason">{preview.reason}</p>}
      <button type="button" title={t('tooltip.files.revealInExplorer')} onClick={reveal}>{t('menu.showInExplorer')}</button>
    </div>
  );
}

interface BufferRecord {
  baseText: string;
  draftText: string;
  diskHash?: string;
}

/**
 * The files pane (M/FilePaneView.swift; WinUI MainWindow.Files.cs) extended with `MC-6`:
 * lightweight syntax-highlighted editing (`⌘S`/`Ctrl+S`), dirty indicator + external modification conflict banner,
 * in-file Find & Replace (`⌘F`/`Ctrl+F`), instant `편집 ↔ Diff` switching, and attaching code selections (`path:start-end`)
 * to the workspace's active agent pane via toolbar pill, right-click menu, or `⇧⌘L` / `Ctrl+Shift+L`.
 */
export function FilesView({ session }: { session: RunSession }) {
  const { bridge, store, t } = useServices();
  const onReferenceClick = useReferenceClick();
  const onReferenceKey = useReferenceKey();
  const paneId = session.id;
  const workspaceId = session.workspaceId;
  const [model, setModel] = useState<FilesModel | null>(null);
  const [filter, setFilter] = useState('');
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [textMode, setTextMode] = useState<TextViewMode>('source');
  const [zoom, setZoom] = useState<number | null>(null);

  // Per-file unsaved buffers kept in memory while the files pane is mounted.
  const [buffers, setBuffers] = useState<Record<string, BufferRecord>>({});
  const buffersRef = useRef(buffers);
  buffersRef.current = buffers;

  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [conflictDiskHash, setConflictDiskHash] = useState<string | null>(null);
  const [conflictDiskText, setConflictDiskText] = useState<string | null>(null);
  const [headChanges, setHeadChanges] = useState<FileChanges | null>(null);
  const [selection, setSelection] = useState<CodeSelection | null>(null);

  // Find & Replace state (`⌘F`).
  const [findOpen, setFindOpen] = useState(false);
  const [showReplace, setShowReplace] = useState(false);
  const [findQuery, setFindQuery] = useState('');
  const [replaceText, setReplaceText] = useState('');
  const [matchCase, setMatchCase] = useState(false);
  const [matchIndex, setMatchIndex] = useState(0);

  const request = useRef(0);
  const filterEdit = useRef(0);
  const lastAgentPaneRef = useRef<string | null>(null);
  const editorTextareaRef = useRef<HTMLTextAreaElement>(null);
  const findInputRef = useRef<HTMLInputElement>(null);

  // Track the most recently active conversation pane in this workspace so selection questions route back to it.
  useEffect(() => {
    const syncActive = () => {
      const snap = store.getState().snapshot;
      if (!snap?.activeSessionId) return;
      const active = snap.sessions.find((s) => s.id === snap.activeSessionId);
      if (active && active.workspaceId === workspaceId && active.kind !== 'files' && active.kind !== 'workmap' && active.kind !== 'shell') {
        lastAgentPaneRef.current = active.id;
      }
    };
    syncActive();
    return store.subscribe(syncActive);
  }, [store, workspaceId]);

  const call = useCallback(<T,>(method: string, args?: object) => bridge.call<T>(method, { paneId, ...args }), [bridge, paneId]);
  const apply = useCallback((value: FilesModel | null | undefined) => { if (value) setModel(value); }, []);

  const currentPath = preview?.path ?? '';
  const currentBuffer = currentPath ? buffers[currentPath] : undefined;
  const activeText = currentBuffer ? currentBuffer.draftText : (preview?.text ?? '');
  const baseText = currentBuffer ? currentBuffer.baseText : (preview?.text ?? '');
  const expectedHash = currentBuffer?.diskHash ?? preview?.diskHash;
  const isDirty = !!currentBuffer && currentBuffer.draftText !== currentBuffer.baseText;
  const dirtyPaths = useMemo(() => {
    const set = new Set<string>();
    for (const [p, buf] of Object.entries(buffers)) {
      if (buf.draftText !== buf.baseText) set.add(p);
    }
    return set;
  }, [buffers]);

  const show = useCallback(async (path: string, debounce: boolean, forceReload = false) => {
    const mine = ++request.current;
    if (debounce) await new Promise((done) => setTimeout(done, 100));
    if (mine !== request.current) return;
    setLoading(true);
    setSaveError(null);
    setConflictDiskHash(null);
    setConflictDiskText(null);
    setSelection(null);
    setHeadChanges(null);
    try {
      const value = await call<FilePreview>('filesPreview', { path });
      if (mine !== request.current) return;
      setPreview(value);
      setZoom(null);
      const existing = buffersRef.current[path];
      const hasDirty = !forceReload && existing && existing.draftText !== existing.baseText;
      if (forceReload && existing) {
        setBuffers((prev) => {
          const copy = { ...prev };
          delete copy[path];
          return copy;
        });
      } else if (hasDirty && value.diskHash && existing.diskHash && value.diskHash !== existing.diskHash) {
        setConflictDiskHash(value.diskHash);
        setConflictDiskText(value.text ?? null);
      }
      if (hasDirty) {
        setTextMode('edit');
      } else if ((value.kind === 'markdown' && value.markdown) || isHtmlPreview(value)) {
        setTextMode('rendered');
      } else {
        setTextMode('source');
      }
    } catch {
      if (mine === request.current) setPreview({ path, name: '', kind: 'unsupported', failure: t('files.preview.failed'), banners: [] });
    } finally {
      if (mine === request.current) setLoading(false);
    }
  }, [call, t]);

  useEffect(() => {
    let live = true;
    void call<FilesModel>('filesState').then((value) => {
      if (!live || !value) return;
      setModel(value); setFilter(value.filter);
      if (value.preview) void show(value.preview, false);
    }, () => {});
    return () => { live = false; };
  }, [call, show]);

  useEffect(() => listenPane('files', paneId, (message) => {
    void call<FilesModel>('filesState').then((value) => { apply(value); if (value) setFilter(value.filter); }, () => {});
    void show(message.path, false);
  }), [call, apply, show, paneId]);

  // Poll disk hash while editing or holding unsaved changes so external agent edits surface a conflict/reload prompt immediately.
  useEffect(() => {
    if (!preview || !preview.editable || !expectedHash || preview.failure) return undefined;
    const targetPath = preview.path;
    let cancelled = false;
    const check = async () => {
      try {
        const res = await call<FilesDiskCheckResult>('filesCheckDisk', { path: targetPath, expectedHash });
        if (cancelled || !res) return;
        if (res.status === 'modified' && res.diskHash && res.diskHash !== expectedHash) {
          if (isDirty) {
            setConflictDiskHash(res.diskHash);
          } else {
            // Clean buffer: silently refresh so the developer always sees the agent's latest changes.
            void show(targetPath, false, true);
          }
        }
      } catch {
        // Ignore transient check errors.
      }
    };
    const interval = setInterval(() => { void check(); }, 2500);
    const onFocus = () => { void check(); };
    window.addEventListener('focus', onFocus);
    return () => {
      cancelled = true;
      clearInterval(interval);
      window.removeEventListener('focus', onFocus);
    };
  }, [call, preview, expectedHash, isDirty, show]);

  // Fetch Git HEAD diff when entering `diff` mode with no unsaved buffer diff (or to inspect disk conflict).
  useEffect(() => {
    if (!preview || preview.failure || textMode !== 'diff') return undefined;
    let live = true;
    if (conflictDiskHash && conflictDiskText === null) {
      void call<FilePreview>('filesPreview', { path: preview.path }).then((fresh) => {
        if (live && fresh?.text !== undefined) setConflictDiskText(fresh.text);
      }, () => {});
    }
    if (!isDirty && !conflictDiskHash) {
      void call<FileChanges | null>('filesChanges', { path: preview.path }).then((changes) => {
        if (live) setHeadChanges(changes);
      }, () => {});
    }
    return () => { live = false; };
  }, [call, preview, textMode, isDirty, conflictDiskHash, conflictDiskText]);

  const updateDraft = useCallback((nextText: string) => {
    if (!preview) return;
    setSaveError(null);
    setBuffers((prev) => {
      const current = prev[preview.path];
      const initialBase = current ? current.baseText : (preview.text ?? '');
      const initialHash = current ? current.diskHash : preview.diskHash;
      return {
        ...prev,
        [preview.path]: {
          baseText: initialBase,
          draftText: nextText,
          diskHash: initialHash,
        },
      };
    });
  }, [preview]);

  const saveCurrent = useCallback(async (force = false) => {
    if (!preview || !preview.editable || saving) return;
    const savingPath = preview.path;
    const textToSave = activeText;
    setSaving(true);
    setSaveError(null);
    try {
      const result = await call<FilesSaveResult>('filesSave', {
        path: savingPath,
        text: textToSave,
        expectedHash,
        force,
      });
      if (!result.ok) {
        if (result.reason === 'conflict') {
          setConflictDiskHash(result.diskHash ?? 'conflict');
          const fresh = await call<FilePreview>('filesPreview', { path: savingPath }).catch(() => null);
          if (fresh?.text !== undefined) setConflictDiskText(fresh.text);
        } else {
          setSaveError(result.message ?? t('files.preview.failed'));
        }
        return;
      }
      setConflictDiskHash(null);
      setConflictDiskText(null);
      setPreview((curr) => (curr && curr.path === savingPath ? {
        ...curr,
        text: textToSave,
        diskHash: result.diskHash ?? curr.diskHash,
        size: result.size ?? curr.size,
        modified: result.modified ?? curr.modified,
        encoding: result.encoding ?? curr.encoding,
        eol: result.eol ?? curr.eol,
      } : curr));
      setBuffers((prev) => {
        const next = { ...prev };
        delete next[savingPath];
        return next;
      });
      setHeadChanges(null);
    } catch (failure) {
      setSaveError(failure instanceof Error ? failure.message : t('files.preview.failed'));
    } finally {
      setSaving(false);
    }
  }, [preview, saving, activeText, call, expectedHash, t]);

  const discardChanges = useCallback(() => {
    if (!preview) return;
    setSaveError(null);
    setBuffers((prev) => {
      const next = { ...prev };
      delete next[preview.path];
      return next;
    });
    if (conflictDiskHash) {
      void show(preview.path, false, true);
    }
  }, [preview, conflictDiskHash, show]);

  // Search & Replace computations.
  const matches = useMemo(
    () => (findOpen && findQuery.length > 0 ? findMatches(activeText, findQuery, matchCase) : []),
    [findOpen, findQuery, activeText, matchCase],
  );
  const safeMatchIndex = matches.length === 0 ? 0 : Math.min(matchIndex, matches.length - 1);

  const revealMatchInEditor = useCallback((index: number, list = matches) => {
    const match = list[index];
    const area = editorTextareaRef.current;
    if (!match || !area) return;
    const totalLines = Math.max(1, activeText.split('\n').length);
    const ratio = (match.line - 1) / totalLines;
    area.scrollTop = Math.max(0, ratio * area.scrollHeight - area.clientHeight * 0.35);
  }, [matches, activeText]);

  const openFind = useCallback((withReplace: boolean) => {
    if (withReplace && preview?.editable && textMode !== 'edit') {
      setTextMode('edit');
    }
    if (selection && !selection.selectedText.includes('\n') && selection.selectedText.length <= 120) {
      setFindQuery(selection.selectedText);
    }
    setFindOpen(true);
    if (withReplace && preview?.editable) setShowReplace(true);
    requestAnimationFrame(() => {
      findInputRef.current?.focus();
      findInputRef.current?.select();
    });
  }, [preview, textMode, selection]);

  const stepMatch = useCallback((delta: -1 | 1) => {
    if (matches.length === 0) return;
    const next = (safeMatchIndex + delta + matches.length) % matches.length;
    setMatchIndex(next);
    revealMatchInEditor(next);
  }, [matches.length, safeMatchIndex, revealMatchInEditor]);

  const onReplaceOne = useCallback(() => {
    if (!preview?.editable || matches.length === 0) return;
    if (textMode !== 'edit') setTextMode('edit');
    const target = matches[safeMatchIndex]!;
    const replaced = replaceSingleMatch(activeText, target, replaceText);
    updateDraft(replaced.text);
    requestAnimationFrame(() => {
      const area = editorTextareaRef.current;
      if (area) area.setSelectionRange(replaced.cursor, replaced.cursor);
    });
  }, [preview, matches, safeMatchIndex, textMode, activeText, replaceText, updateDraft]);

  const onReplaceAll = useCallback(() => {
    if (!preview?.editable || matches.length === 0) return;
    if (textMode !== 'edit') setTextMode('edit');
    const replaced = replaceAllMatches(activeText, findQuery, replaceText, matchCase);
    if (replaced.count > 0) {
      updateDraft(replaced.text);
      setMatchIndex(0);
    }
  }, [preview, matches.length, textMode, activeText, findQuery, replaceText, matchCase, updateDraft]);

  // Attach `path:start-end` + fenced snippet to the workspace's target agent composer and focus it.
  const sendSelectionToAgent = useCallback((sel: CodeSelection, kind: 'ask' | 'edit') => {
    if (!preview) return;
    const snap = store.getState().snapshot;
    const target = pickTargetAgentSession(
      snap?.sessions ?? [],
      workspaceId,
      snap?.activeSessionId,
      lastAgentPaneRef.current,
    );
    if (!target) {
      setSaveError(t('files.editor.noAgentPane'));
      return;
    }
    const prefix = kind === 'edit' ? t('files.editor.requestEditPrefix') : t('files.editor.askAgentPrefix');
    const block = formatAgentSelectionBlock(preview.path, sel.startLine, sel.endLine, sel.selectedText, prefix);
    const merged = appendingBlock(block, target.draft ?? '');
    void bridge.call('updateDraft', { paneId: target.id, text: merged }).catch(() => {});
    store.dispatch({ kind: 'fill', paneId: target.id, text: block, append: true });
    store.dispatch({ kind: 'choose', paneId: target.id });
    void bridge.call('activatePane', { id: target.id }).catch(() => {});
  }, [preview, store, workspaceId, bridge, t]);

  const editor = useCallback((path: string) => void call('openInEditor', { source: 'files', path }).catch(() => {}), [call]);
  const menu = useMenu();
  const menuRef = useRef(menu);
  menuRef.current = menu;

  const onCodeContextMenu = useCallback((event: ReactMouseEvent, sel: CodeSelection | null) => {
    if (!preview || !sel) return;
    const refLabel = formatSelectionReference(preview.path, sel.startLine, sel.endLine);
    menuRef.current.atPointer(event, [
      {
        label: t('files.editor.askSelectionMenu', { ref: refLabel }),
        icon: <Icon name="message-square-text" size={12} />,
        action: () => sendSelectionToAgent(sel, 'ask'),
        testId: 'files-menu-ask-agent',
      },
      {
        label: t('files.editor.editSelectionMenu', { ref: refLabel }),
        icon: <Icon name="sparkles" size={12} />,
        action: () => sendSelectionToAgent(sel, 'edit'),
        testId: 'files-menu-edit-agent',
      },
      ...(preview.editable && textMode !== 'edit' ? [{
        label: t('files.editor.modeEdit'),
        icon: <Icon name="pencil-line" size={12} />,
        action: () => setTextMode('edit'),
      }] : []),
      {
        label: t('editor.open'),
        icon: <Icon name="file-code" size={12} />,
        action: () => editor(preview.path),
      },
    ]);
  }, [preview, textMode, sendSelectionToAgent, editor, t]);

  // Pane-level keyboard shortcuts when focus is inside the preview column (`⌘F`, `⌘S`, `⇧⌘L`).
  const onPreviewKeyDown = useCallback((event: KeyboardEvent<HTMLDivElement>) => {
    if (!preview || preview.failure) return;
    const mod = event.metaKey || event.ctrlKey;
    const lower = event.key.toLowerCase();
    if (mod && !event.shiftKey && lower === 'f' && (preview.kind === 'source' || preview.kind === 'markdown')) {
      event.preventDefault();
      openFind(event.altKey);
      return;
    }
    if (mod && !event.altKey && !event.shiftKey && lower === 's' && preview.editable && isDirty) {
      event.preventDefault();
      void saveCurrent(false);
      return;
    }
    if (mod && event.shiftKey && lower === 'l' && selection) {
      event.preventDefault();
      sendSelectionToAgent(selection, 'ask');
    }
  }, [preview, openFind, isDirty, saveCurrent, selection, sendSelectionToAgent]);

  const choose = useCallback((row: FileRow) => {
    void call<FilesModel>('filesSelect', { path: row.path }).then(apply, () => {});
    setModel((current) => (current ? { ...current, selected: row.path } : current));
    if (!row.dir) void show(row.path, true);
  }, [call, apply, show]);
  const filtering = model?.filtering ?? false;
  const onRow = useCallback((row: FileRow) => {
    if (row.dir) {
      if (filtering) void call<FilesModel>('filesReveal', { path: row.path }).then((value) => { apply(value); if (value) setFilter(value.filter); }, () => {});
      else void call<FilesModel>('filesToggle', { path: row.path }).then(apply, () => {});
      setModel((current) => (current ? { ...current, selected: row.path } : current));
      return;
    }
    choose(row);
  }, [filtering, call, apply, choose]);
  const onKey = useCallback((event: KeyboardEvent<HTMLDivElement>) => {
    if (!model) return;
    const action = treeKey(model.rows, model.selected, event.key, model.filtering);
    if (action.kind === 'none') return;
    event.preventDefault();
    const row = model.rows.find((candidate) => candidate.path === action.path);
    if (action.kind === 'select') { if (row) choose(row); else setModel({ ...model, selected: action.path }); }
    else if (action.kind === 'toggle') void call<FilesModel>('filesToggle', { path: action.path }).then(apply, () => {});
    else void call<FilesModel>('filesReveal', { path: action.path }).then((value) => { apply(value); if (value) setFilter(value.filter); }, () => {});
  }, [model, call, apply, choose]);
  const onFilter = (text: string) => {
    setFilter(text);
    const edit = ++filterEdit.current;
    setTimeout(() => { if (edit === filterEdit.current) void call<FilesModel>('filesFilter', { text }).then(apply, () => {}); }, 100);
  };
  const reveal = () => { if (preview) void call('filesShowInExplorer', { path: preview.path }).catch(() => {}); };
  const open = () => { if (preview) void call('filesOpen', { path: preview.path }).catch(() => {}); };

  const onMenu = useCallback((event: ReactMouseEvent, row: FileRow) => {
    menuRef.current.atPointer(event, [
      { label: t(row.dir ? 'editor.openFolder' : 'editor.open'), icon: <Icon name="file-code" size={12} />, action: () => editor(row.path), testId: 'files-menu-editor' },
      ...(row.dir ? [] : [{ label: t('graph.resultFiles.openFile', { path: row.name }), action: () => void call('filesOpen', { path: row.path }).catch(() => {}) }]),
      { label: t('menu.showInExplorer'), action: () => void call('filesShowInExplorer', { path: row.path }).catch(() => {}) },
    ]);
  }, [call, editor, t]);

  const diffToRender = useMemo<FileChanges | null>(() => {
    if (textMode !== 'diff' || !preview) return null;
    if (conflictDiskHash && conflictDiskText !== null) {
      return computeBufferChanges(conflictDiskText, activeText);
    }
    if (isDirty) {
      return computeBufferChanges(baseText, activeText);
    }
    return headChanges;
  }, [textMode, preview, conflictDiskHash, conflictDiskText, activeText, isDirty, baseText, headChanges]);

  if (model && !model.available) return <div className="files-pane files-unavailable" data-testid="files-pane" />;
  const shows = preview && !preview.failure;
  const isTextPreview = shows && (preview.kind === 'source' || preview.kind === 'markdown');
  const hasRenderedView = shows && ((preview.kind === 'markdown' && !!preview.markdown) || isHtmlPreview(preview));
  const markdownRendered = shows && preview.kind === 'markdown' && !!preview.markdown && textMode === 'rendered';
  const htmlPage = shows && isHtmlPreview(preview) && textMode === 'rendered';
  const banners = markdownRendered ? preview.renderedBanners ?? [] : htmlPage || textMode === 'diff' ? [] : preview?.banners ?? [];
  const image = preview?.kind === 'image' ? preview.image : undefined;
  const step = (factor: number) => setZoom((current) => Math.min(8, Math.max(0.05, (current ?? 1) * factor)));
  const selectionRangeLabel = selection
    ? selection.startLine === selection.endLine
      ? `L${selection.startLine}`
      : `L${selection.startLine}-${selection.endLine}`
    : null;

  return (
    <div className="files-pane" data-testid="files-pane">
      <div className="files-tree-column">
        <div className="files-filter-row">
          <svg className="files-filter-symbol" viewBox="0 0 12 12" aria-hidden="true"><path d="M1.5 3h9M3 6h6M4.5 9h3" /></svg>
          <input className="files-filter" data-testid="files-filter" value={filter} placeholder={t('files.tree.filter')} title={t('tooltip.files.filter')} aria-label={t('files.tree.filter')} spellCheck={false} onChange={(event) => onFilter(event.target.value)} />
          <button type="button" className="files-tool" aria-label={t('files.tree.refresh')} title={t('files.tree.refresh')}
            onClick={() => { void call<FilesModel>('filesRefresh').then(apply, () => {}); if (preview?.path) void show(preview.path, false); }}>
            <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M10 6a4 4 0 1 1-1.2-2.85M10 1.5v2.5H7.5" /></svg>
          </button>
        </div>
        {model && <Tree model={model} dirtyPaths={dirtyPaths} onRow={onRow} onKey={onKey} onMenu={onMenu} />}
      </div>
      <div
        className="files-preview-column"
        data-testid="files-preview"
        data-preview-path={preview?.path ?? ''}
        data-preview-kind={preview?.failure ? 'failed' : preview?.kind ?? ''}
        tabIndex={-1}
        onKeyDown={onPreviewKeyDown}
      >
        {shows && (
          <div className="files-preview-head">
            <Symbol symbol={preview.kind} />
            <div className="files-preview-names">
              <div className="files-preview-title-row">
                <span className="files-preview-title">{preview.name}</span>
                {isDirty && (
                  <span
                    className="files-dirty-dot"
                    data-testid="files-dirty-indicator"
                    title={t('files.editor.unsavedChanges')}
                    aria-label={t('files.editor.unsavedChanges')}
                  />
                )}
              </div>
              <MiddlePath className="files-preview-path" path={preview.path} />
            </div>
            <div className="files-preview-tools">
              {selection && isTextPreview && (
                <button
                  type="button"
                  className="files-action-pill accent-soft"
                  data-testid="files-ask-selection"
                  title={t('files.editor.askSelectionTooltip')}
                  onClick={() => sendSelectionToAgent(selection, 'ask')}
                >
                  <Icon name="message-square-text" size={11} weight={2} />
                  <span>{t('files.editor.askSelectionShort', { lines: selectionRangeLabel ?? '' })}</span>
                </button>
              )}
              {isDirty && (
                <>
                  <button
                    type="button"
                    className="files-action-pill"
                    data-testid="files-discard"
                    disabled={saving}
                    title={t('files.editor.discard')}
                    onClick={discardChanges}
                  >
                    {t('files.editor.discard')}
                  </button>
                  <button
                    type="button"
                    className="files-action-pill primary"
                    data-testid="files-save"
                    disabled={saving}
                    title={t('files.editor.saveShortcut')}
                    onClick={() => void saveCurrent(false)}
                  >
                    {saving ? t('files.editor.saving') : t('files.editor.save')}
                  </button>
                </>
              )}
              {image && <span className="files-pixels">{image.label}</span>}
              {image && (
                <>
                  <button type="button" className="files-tool" disabled={!image.zoomable} aria-label={t('files.image.zoomOut')} title={t('files.image.zoomOut')} onClick={() => step(1 / 1.25)}>−</button>
                  <button type="button" className="files-tool" disabled={!image.zoomable} aria-label={t('files.image.fit')} title={t('files.image.fit')} onClick={() => setZoom(null)}>⤢</button>
                  <button type="button" className="files-tool files-tool-word" disabled={!image.zoomable} aria-label={t('files.image.actualSize')} title={t('files.image.actualSize')} onClick={() => setZoom(1)}>1:1</button>
                  <button type="button" className="files-tool" disabled={!image.zoomable} aria-label={t('files.image.zoomIn')} title={t('files.image.zoomIn')} onClick={() => step(1.25)}>+</button>
                </>
              )}
              {preview.encoding && <span className="files-encoding" title={t('files.preview.encoding')}>{preview.encoding}</span>}
              {isTextPreview && (
                <div className="files-segment" role="group" data-testid="files-mode-switch">
                  {hasRenderedView && (
                    <button
                      type="button"
                      className={textMode === 'rendered' ? 'chosen' : ''}
                      title={t(isHtmlPreview(preview) ? 'tooltip.files.htmlPage' : 'tooltip.files.showRendered')}
                      aria-pressed={textMode === 'rendered'}
                      onClick={() => setTextMode('rendered')}
                    >
                      {t(isHtmlPreview(preview) ? 'files.html.page' : 'files.markdown.rendered')}
                    </button>
                  )}
                  <button
                    type="button"
                    className={textMode === 'source' ? 'chosen' : ''}
                    title={t('tooltip.files.showSource')}
                    aria-pressed={textMode === 'source'}
                    onClick={() => setTextMode('source')}
                  >
                    {t('files.editor.modeView')}
                  </button>
                  {preview.editable && (
                    <button
                      type="button"
                      className={textMode === 'edit' ? 'chosen' : ''}
                      data-testid="files-mode-edit"
                      title={t('files.editor.modeEditTooltip')}
                      aria-pressed={textMode === 'edit'}
                      onClick={() => {
                        setTextMode('edit');
                        requestAnimationFrame(() => editorTextareaRef.current?.focus());
                      }}
                    >
                      {t('files.editor.modeEdit')}
                    </button>
                  )}
                  <button
                    type="button"
                    className={textMode === 'diff' ? 'chosen' : ''}
                    data-testid="files-mode-diff"
                    title={t('files.editor.modeDiffTooltip')}
                    aria-pressed={textMode === 'diff'}
                    onClick={() => setTextMode('diff')}
                  >
                    {t('files.editor.modeDiff')}
                  </button>
                </div>
              )}
              {isTextPreview && (
                <button
                  type="button"
                  className={'files-tool' + (findOpen ? ' active' : '')}
                  data-testid="files-find-toggle"
                  aria-pressed={findOpen}
                  aria-label={t('files.find.placeholder')}
                  title={t('files.find.shortcut')}
                  onClick={() => {
                    if (findOpen) setFindOpen(false);
                    else openFind(false);
                  }}
                >
                  <Icon name="search" size={13} weight={1.8} />
                </button>
              )}
              <button type="button" className="files-tool" data-testid="files-editor" aria-label={t('editor.open')} title={t('editor.open')} onClick={() => editor(preview.path)}>
                <Icon name="file-code" size={13} weight={1.7} />
              </button>
              <button type="button" className="files-tool" aria-label={t('graph.resultFiles.openFile', { path: preview.name })} title={t('graph.resultFiles.openFile', { path: preview.name })} onClick={open}>
                <svg viewBox="0 0 14 14" aria-hidden="true"><path d="M8 2h4v4M12 2 6.5 7.5M10.5 8.5V12h-8.5V3.5H5.5" /></svg>
              </button>
              <button type="button" className="files-tool" data-testid="files-reveal" aria-label={t('menu.showInExplorer')} title={t('tooltip.files.revealInExplorer')} onClick={reveal}>
                <svg viewBox="0 0 14 14" aria-hidden="true"><path d="M1.5 3.5h4l1.2 1.3h5.8v6.7h-11z" /></svg>
              </button>
            </div>
          </div>
        )}
        {conflictDiskHash && shows && (
          <div className="files-conflict-banner" data-testid="files-conflict-banner" role="alert">
            <Icon name="triangle-alert" size={13} weight={2} />
            <span>{t('files.editor.conflictBanner')}</span>
            <span className="files-conflict-actions">
              <button type="button" className="files-action-pill" onClick={() => setTextMode('diff')}>
                {t('files.editor.conflictCompare')}
              </button>
              <button type="button" className="files-action-pill" onClick={() => void show(preview.path, false, true)}>
                {t('files.editor.conflictReload')}
              </button>
              <button type="button" className="files-action-pill primary" onClick={() => void saveCurrent(true)}>
                {t('files.editor.conflictOverwrite')}
              </button>
            </span>
          </div>
        )}
        {saveError && (
          <div className="files-error-banner" data-testid="files-save-error" role="alert">
            <span>{saveError}</span>
            <button type="button" className="files-tool" aria-label={t('settings.closeButton')} onClick={() => setSaveError(null)}>
              <Icon name="x" size={12} weight={2.2} />
            </button>
          </div>
        )}
        {findOpen && isTextPreview && (
          <FindReplaceBar
            query={findQuery}
            replacement={replaceText}
            caseSensitive={matchCase}
            showReplace={showReplace}
            canReplace={!!preview?.editable}
            matchCount={matches.length}
            currentIndex={safeMatchIndex}
            inputRef={findInputRef}
            onQueryChange={(q) => { setFindQuery(q); setMatchIndex(0); }}
            onReplacementChange={setReplaceText}
            onToggleCase={() => { setMatchCase((c) => !c); setMatchIndex(0); }}
            onToggleReplace={() => {
              if (!showReplace && textMode !== 'edit' && preview?.editable) setTextMode('edit');
              setShowReplace((s) => !s);
            }}
            onStep={stepMatch}
            onReplaceOne={onReplaceOne}
            onReplaceAll={onReplaceAll}
            onClose={() => setFindOpen(false)}
          />
        )}
        {banners.map((banner) => <div key={banner} className="files-banner">{banner}</div>)}
        <div className="files-preview-body">
          {loading && !preview && <div className="files-progress" />}
          {!preview && !loading && <div className="files-placeholder">{t('files.preview.placeholder')}</div>}
          {preview?.failure && <div className="files-placeholder" data-testid="files-failure">{preview.failure}</div>}
          {shows && isTextPreview && textMode === 'diff' && (
            diffToRender && !diffToRender.unchanged && diffToRender.hunks.length > 0 ? (
              <DiffView changes={diffToRender} name={preview.path} />
            ) : (
              <div className="files-placeholder" data-testid="files-diff-empty">{t('files.editor.noDiff')}</div>
            )
          )}
          {shows && isTextPreview && textMode === 'edit' && preview.editable && (
            <CodeEditor
              path={preview.path}
              value={activeText}
              wraps={!!preview.wraps}
              matches={matches}
              currentMatchIndex={safeMatchIndex}
              textareaRef={editorTextareaRef}
              onChange={updateDraft}
              onSave={() => void saveCurrent(false)}
              onOpenFind={openFind}
              onSelectionChange={setSelection}
              onAskSelection={(sel) => sendSelectionToAgent(sel, 'ask')}
              onContextMenuSelection={onCodeContextMenu}
            />
          )}
          {shows && preview.kind === 'source' && textMode !== 'edit' && textMode !== 'diff' && (
            htmlPage ? (
              <HtmlFrame paneId={paneId} path={preview.path} name={preview.name} reference={false} />
            ) : (
              <SourceView
                preview={{ ...preview, text: activeText }}
                onSelectionChange={setSelection}
                onContextMenuSelection={onCodeContextMenu}
              />
            )
          )}
          {shows && preview.kind === 'markdown' && textMode !== 'edit' && textMode !== 'diff' && (
            markdownRendered ? (
              <div className="files-markdown" onClick={onReferenceClick} onKeyDown={onReferenceKey}>
                <Markdown text={activeText} />
              </div>
            ) : (
              <SourceView
                preview={{ ...preview, text: activeText }}
                onSelectionChange={setSelection}
                onContextMenuSelection={onCodeContextMenu}
              />
            )
          )}
          {shows && preview.kind === 'image' && (image ? <ImageView preview={preview} zoom={zoom} /> : <Unsupported preview={preview} reveal={reveal} />)}
          {shows && preview.kind === 'unsupported' && <Unsupported preview={preview} reveal={reveal} />}
        </div>
      </div>
    </div>
  );
}

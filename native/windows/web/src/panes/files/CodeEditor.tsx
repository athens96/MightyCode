import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from 'react';
import { useServices } from '../../app/context';
import { Icon } from '../../icons/Icon';
import {
  indentSelection,
  newlineWithIndent,
  renderEditorBackdropSegments,
  selectionLineSpan,
  type CodeSelection,
  type SearchMatch,
} from './editorModel';

export function FindReplaceBar({
  query,
  replacement,
  caseSensitive,
  showReplace,
  canReplace,
  matchCount,
  currentIndex,
  inputRef,
  onQueryChange,
  onReplacementChange,
  onToggleCase,
  onToggleReplace,
  onStep,
  onReplaceOne,
  onReplaceAll,
  onClose,
}: {
  query: string;
  replacement: string;
  caseSensitive: boolean;
  showReplace: boolean;
  canReplace: boolean;
  matchCount: number;
  currentIndex: number;
  inputRef: React.RefObject<HTMLInputElement | null>;
  onQueryChange: (value: string) => void;
  onReplacementChange: (value: string) => void;
  onToggleCase: () => void;
  onToggleReplace: () => void;
  onStep: (delta: -1 | 1) => void;
  onReplaceOne: () => void;
  onReplaceAll: () => void;
  onClose: () => void;
}) {
  const { t } = useServices();
  const countLabel = query.length === 0
    ? ''
    : matchCount === 0
      ? t('files.find.noMatches')
      : `${currentIndex + 1} / ${matchCount}`;

  return (
    <div className="files-find-bar" data-testid="files-find-bar" onKeyDown={(event) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose(); }
    }}>
      <div className="files-find-row">
        {canReplace && (
          <button
            type="button"
            className="files-tool"
            aria-expanded={showReplace}
            aria-label={t('files.find.toggleReplace')}
            title={t('files.find.toggleReplace')}
            onClick={onToggleReplace}
          >
            <Icon name={showReplace ? 'chevron-down' : 'chevron-right'} size={11} weight={2.4} />
          </button>
        )}
        <Icon name="search" size={12} weight={2} className="files-find-glyph" />
        <input
          ref={inputRef}
          className="files-find-input"
          data-testid="files-find-input"
          value={query}
          placeholder={t('files.find.placeholder')}
          aria-label={t('files.find.placeholder')}
          spellCheck={false}
          onChange={(event) => onQueryChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              onStep(event.shiftKey ? -1 : 1);
            }
          }}
        />
        {countLabel && <span className={'files-find-count' + (matchCount === 0 ? ' empty' : '')}>{countLabel}</span>}
        <button
          type="button"
          className={'files-tool files-tool-word' + (caseSensitive ? ' active' : '')}
          aria-pressed={caseSensitive}
          title={t('files.find.matchCase')}
          onClick={onToggleCase}
        >
          Aa
        </button>
        <button type="button" className="files-tool" disabled={matchCount === 0} aria-label={t('files.find.previous')} title={t('files.find.previous')} onClick={() => onStep(-1)}>
          <Icon name="chevron-up" size={12} weight={2.2} />
        </button>
        <button type="button" className="files-tool" disabled={matchCount === 0} aria-label={t('files.find.next')} title={t('files.find.next')} onClick={() => onStep(1)}>
          <Icon name="chevron-down" size={12} weight={2.2} />
        </button>
        <button type="button" className="files-tool" aria-label={t('settings.closeButton')} title={t('settings.closeButton')} onClick={onClose}>
          <Icon name="x" size={12} weight={2.2} />
        </button>
      </div>
      {canReplace && showReplace && (
        <div className="files-find-row files-replace-row">
          <span className="files-replace-spacer" aria-hidden="true" />
          <input
            className="files-find-input"
            data-testid="files-replace-input"
            value={replacement}
            placeholder={t('files.find.replacePlaceholder')}
            aria-label={t('files.find.replacePlaceholder')}
            spellCheck={false}
            onChange={(event) => onReplacementChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                if (event.metaKey || event.ctrlKey) onReplaceAll();
                else onReplaceOne();
              }
            }}
          />
          <button type="button" className="files-action-pill" data-testid="files-replace-one" disabled={matchCount === 0} onClick={onReplaceOne}>
            {t('files.find.replace')}
          </button>
          <button type="button" className="files-action-pill" data-testid="files-replace-all" disabled={matchCount === 0} onClick={onReplaceAll}>
            {t('files.find.replaceAll')}
          </button>
        </div>
      )}
    </div>
  );
}

export function CodeEditor({
  path,
  value,
  wraps,
  matches,
  currentMatchIndex,
  textareaRef,
  onChange,
  onSave,
  onOpenFind,
  onSelectionChange,
  onAskSelection,
  onContextMenuSelection,
}: {
  path: string;
  value: string;
  wraps: boolean;
  matches: readonly SearchMatch[];
  currentMatchIndex: number;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  onChange: (next: string) => void;
  onSave: () => void;
  onOpenFind: (withReplace: boolean) => void;
  onSelectionChange: (selection: CodeSelection | null) => void;
  onAskSelection: (selection: CodeSelection) => void;
  onContextMenuSelection: (event: ReactMouseEvent, selection: CodeSelection | null) => void;
}) {
  const gutterRef = useRef<HTMLDivElement>(null);
  const backdropRef = useRef<HTMLPreElement>(null);
  const [activeLine, setActiveLine] = useState(1);

  const lineCount = useMemo(() => {
    let count = 1;
    for (let i = 0; i < value.length; i++) {
      if (value.charCodeAt(i) === 10) count++;
    }
    return count;
  }, [value]);

  const segments = useMemo(
    () => renderEditorBackdropSegments(value, path, matches, currentMatchIndex),
    [value, path, matches, currentMatchIndex],
  );

  const syncScroll = useCallback(() => {
    const area = textareaRef.current;
    if (!area) return;
    if (gutterRef.current) gutterRef.current.scrollTop = area.scrollTop;
    if (backdropRef.current) {
      backdropRef.current.scrollTop = area.scrollTop;
      backdropRef.current.scrollLeft = area.scrollLeft;
    }
  }, [textareaRef]);

  const updateSelection = useCallback(() => {
    const area = textareaRef.current;
    if (!area) return;
    const caret = area.selectionStart ?? 0;
    let line = 1;
    for (let i = 0; i < caret && i < value.length; i++) {
      if (value.charCodeAt(i) === 10) line++;
    }
    setActiveLine(line);
    onSelectionChange(selectionLineSpan(value, area.selectionStart ?? 0, area.selectionEnd ?? 0));
  }, [textareaRef, value, onSelectionChange]);

  useEffect(() => {
    syncScroll();
  }, [value, wraps, syncScroll]);

  const onKeyDown = (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    const mod = event.metaKey || event.ctrlKey;
    const lower = event.key.toLowerCase();
    if (mod && !event.altKey && !event.shiftKey && lower === 's') {
      event.preventDefault();
      onSave();
      return;
    }
    if (mod && !event.shiftKey && lower === 'f') {
      event.preventDefault();
      onOpenFind(event.altKey);
      return;
    }
    if (mod && !event.altKey && lower === 'h') {
      event.preventDefault();
      onOpenFind(true);
      return;
    }
    if (mod && event.shiftKey && lower === 'l') {
      const area = event.currentTarget;
      const span = selectionLineSpan(value, area.selectionStart, area.selectionEnd);
      if (span) {
        event.preventDefault();
        onAskSelection(span);
      }
      return;
    }
    if (event.key === 'Tab' && !mod && !event.altKey) {
      event.preventDefault();
      const area = event.currentTarget;
      const res = indentSelection(value, area.selectionStart, area.selectionEnd, event.shiftKey);
      onChange(res.text);
      requestAnimationFrame(() => {
        area.focus();
        area.setSelectionRange(res.start, res.end);
        updateSelection();
      });
      return;
    }
    if (event.key === 'Enter' && !mod && !event.altKey && !event.shiftKey) {
      event.preventDefault();
      const area = event.currentTarget;
      const res = newlineWithIndent(value, area.selectionStart, area.selectionEnd);
      onChange(res.text);
      requestAnimationFrame(() => {
        area.focus();
        area.setSelectionRange(res.cursor, res.cursor);
        updateSelection();
        syncScroll();
      });
    }
  };

  return (
    <div className={'files-editor' + (wraps ? ' wraps' : '')} data-testid="files-code-editor">
      <div className="files-editor-gutter" ref={gutterRef} aria-hidden="true">
        {Array.from({ length: lineCount }, (_, idx) => {
          const line = idx + 1;
          return (
            <div key={line} className={'files-editor-line-no' + (line === activeLine ? ' active' : '')}>
              {line}
            </div>
          );
        })}
      </div>
      <div className="files-editor-stage">
        <pre className="files-editor-backdrop" ref={backdropRef} aria-hidden="true">
          {segments.map((seg, idx) => {
            const cls =
              (seg.token !== 'plain' ? 'tok-' + seg.token : '') +
              (seg.match === 'current' ? ' files-find-mark current' : seg.match === 'match' ? ' files-find-mark' : '');
            return cls ? <span key={idx} className={cls.trim()}>{seg.text}</span> : seg.text;
          })}
          {value.endsWith('\n') ? ' ' : ''}
        </pre>
        <textarea
          ref={textareaRef}
          className="files-editor-textarea"
          data-testid="files-editor-textarea"
          value={value}
          spellCheck={false}
          autoCapitalize="off"
          autoComplete="off"
          autoCorrect="off"
          wrap={wraps ? 'soft' : 'off'}
          onChange={(event) => {
            onChange(event.target.value);
            requestAnimationFrame(updateSelection);
          }}
          onKeyDown={onKeyDown}
          onScroll={syncScroll}
          onSelect={updateSelection}
          onKeyUp={updateSelection}
          onMouseUp={updateSelection}
          onContextMenu={(event) => {
            const area = event.currentTarget;
            const span = selectionLineSpan(value, area.selectionStart, area.selectionEnd);
            onContextMenuSelection(event, span);
          }}
        />
      </div>
    </div>
  );
}

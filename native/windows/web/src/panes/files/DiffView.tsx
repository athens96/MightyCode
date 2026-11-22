import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { PatchDiff, type DiffLineAnnotation } from '@pierre/diffs/react';
import { useServices } from '../../app/context';
import { baseCaption, diffRows, hunkHeader, placeOf, rowOf, unifiedPatch, type DiffRow } from './diff';
import { canAct, isHandled, RefusalChip, RefusalText, type HunkAction, type HunkState } from './hunkReview';
import type { FileChanges } from './types';
import { useAppTheme } from './useAppTheme';

/** Where the chosen layout is kept between views and launches. */
const LAYOUT_KEY = 'mc.diff.layout';

function savedLayout(): 'split' | 'unified' {
  try {
    return localStorage.getItem(LAYOUT_KEY) === 'split' ? 'split' : 'unified';
  } catch {
    return 'unified';
  }
}

/** 바뀐 부분별 받기·버리기: each hunk drawn on its own under a head with 받기 ✓ and 버리기 ↶ (hunkReview.ts). */
export interface HunkControls {
  /** Each hunk's state, in the order of `changes.hunks`. */
  states: readonly HunkState[];
  /** The hunk the keys act on. */
  current: number;
  busy: boolean;
  onCurrent(index: number): void;
  onAct(action: HunkAction, index: number): void;
}

/**
 * The 변경사항 view of a source preview and of 요청 변경 검토 (M/FileDiffView.swift, the phone's DiffView): a caption naming the
 * base and the counts, a 한 줄로 | 나란히 toggle, the truncated note, then the diff drawn by @pierre/diffs from the host's hunks: the
 * source's syntax colours (Shiki, the same as VS Code's), the words that changed inside a line marked, unchanged stretches folded
 * between hunks. A click on a line picks it for a note and a right-click opens the file there; both name the line by its index in
 * `diffRows`, as before. Memoised on the change set: a bubble resized by its grip re-renders with the same object.
 */
export const DiffView = memo(function DiffView({ changes, name, onLine, onOpenLine, noted, picked, hunks }: {
  changes: FileChanges;
  /** The file's name or path: the renderer picks the source's language from it. */
  name?: string;
  /** 편집기에서 열기: a right-click on a line opens the file there; the rows' indexes are `diffRows`'. */
  onOpenLine?: (row: DiffRow, index: number) => void;
  /** 요청 변경 검토: a click on a line picks it for a note; the rows' indexes are `diffRows`'. */
  onLine?: (row: DiffRow, index: number) => void;
  /** Rows that carry a note. */
  noted?: ReadonlySet<number>;
  /** The row a note is being written on. */
  picked?: number | null;
  /** 바뀐 부분별 받기·버리기: the hunks drawn one by one with their heads. */
  hunks?: HunkControls;
}) {
  const { t } = useServices();
  const theme = useAppTheme();
  const [layout, setLayout] = useState(savedLayout);
  const rows = useMemo(() => diffRows(changes), [changes]);
  const patch = useMemo(() => unifiedPatch(name ?? 'file', changes), [name, changes]);
  // The line under the pointer, for a right-click (the renderer reports hovers, not context menus).
  const hovered = useRef<number>(-1);
  const choose = (next: 'split' | 'unified') => {
    setLayout(next);
    try {
      localStorage.setItem(LAYOUT_KEY, next);
    } catch {
      // A page without storage keeps the choice for this view only.
    }
  };
  const annotations = useMemo<DiffLineAnnotation<number>[]>(() => {
    const marks: DiffLineAnnotation<number>[] = [];
    for (const index of noted ?? []) {
      const place = rows[index] && placeOf(rows[index]);
      if (place) marks.push({ ...place, metadata: index });
    }
    return marks;
  }, [noted, rows]);
  const pickedPlace = picked !== null && picked !== undefined && rows[picked] ? placeOf(rows[picked]) : null;
  const options = useMemo(() => ({
    theme: { dark: 'pierre-dark', light: 'pierre-light' },
    themeType: theme,
    diffStyle: layout,
    lineDiffType: 'word-alt' as const,
    hunkSeparators: 'line-info-basic' as const,
    overflow: 'scroll' as const,
    disableFileHeader: true,
    lineHoverHighlight: 'both' as const,
    onLineClick: onLine ? ({ lineNumber, annotationSide }: { lineNumber: number; annotationSide: 'deletions' | 'additions' }) => {
      const index = rowOf(rows, annotationSide, lineNumber);
      if (index >= 0) onLine(rows[index]!, index);
    } : undefined,
    onLineEnter: ({ lineNumber, annotationSide }: { lineNumber: number; annotationSide: 'deletions' | 'additions' }) => {
      hovered.current = rowOf(rows, annotationSide, lineNumber);
    },
    onLineLeave: () => {
      hovered.current = -1;
    },
  }), [theme, layout, onLine, rows]);
  const contextMenu = useCallback((event: React.MouseEvent) => {
    const index = hovered.current;
    if (!onOpenLine || index < 0 || !rows[index]) return;
    event.preventDefault();
    onOpenLine(rows[index]!, index);
  }, [onOpenLine, rows]);
  const patches = useMemo(() => (hunks ? changes.hunks.map((hunk) => unifiedPatch(name ?? 'file', { hunks: [hunk], created: changes.created })) : []),
    [hunks, name, changes]);
  const selected = pickedPlace ? { start: pickedPlace.lineNumber, end: pickedPlace.lineNumber, side: pickedPlace.side } : null;
  const renderNote = () => <div className="files-diff-note-mark">{t('runChanges.notes.title', { count: 1 })}</div>;
  const base = baseCaption(changes);
  const caption = base === 'created' ? t('files.changes.created')
    : base === 'run' ? t('files.changes.baseRun')
    : base === 'head' ? t('files.changes.baseHead', { sha: changes.headSha ?? '' })
    : t('files.changes.baseHeadPlain');
  return (
    <div className="files-diff" data-testid="files-diff">
      <div className="files-diff-caption">
        <span>{caption}</span>
        <span className="files-diff-count added" title={t('tooltip.diff.counts', { added: changes.added, removed: changes.removed })}>+{changes.added}</span>
        <span className="files-diff-count removed" title={t('tooltip.diff.counts', { added: changes.added, removed: changes.removed })}>−{changes.removed}</span>
        <span className="files-diff-layout" role="group" aria-label={t('files.changes.layout')}>
          <button type="button" className={layout === 'unified' ? 'chosen' : ''} title={t('tooltip.diff.unified')} aria-pressed={layout === 'unified'} onClick={() => choose('unified')}>{t('files.changes.unified')}</button>
          <button type="button" className={layout === 'split' ? 'chosen' : ''} title={t('tooltip.diff.split')} aria-pressed={layout === 'split'} onClick={() => choose('split')}>{t('files.changes.split')}</button>
        </span>
      </div>
      {changes.truncated && <div className="files-banner">{t('files.changes.truncated')}</div>}
      <div className={'files-diff-body' + (onLine ? ' notable' : '')} onContextMenu={contextMenu}>
        {hunks ? <HunkList changes={changes} patches={patches} controls={hunks} options={options} annotations={annotations} selected={selected} renderNote={renderNote} />
          : <PatchDiff<number> patch={patch} options={options} lineAnnotations={annotations} selectedLines={selected} renderAnnotation={renderNote} />}
      </div>
    </div>
  );
});

/**
 * The hunks one under another, each under its head: its `@@` place, its state (받음, or why it cannot be put back) and 받기 ✓ /
 * 버리기 ↶. A hunk taken is dimmed and folded to its head (a click on the head opens it again); the current one is outlined and
 * scrolled into view.
 */
function HunkList({ changes, patches, controls, options, annotations, selected, renderNote }: {
  changes: FileChanges;
  patches: string[];
  controls: HunkControls;
  options: Parameters<typeof PatchDiff<number>>[0]['options'];
  annotations: DiffLineAnnotation<number>[];
  selected: { start: number; end: number; side: 'deletions' | 'additions' } | null;
  renderNote: () => React.ReactNode;
}) {
  const { t } = useServices();
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
  const heads = useRef<(HTMLElement | null)[]>([]);
  const { states, current, busy, onCurrent, onAct } = controls;
  useEffect(() => { heads.current[current]?.scrollIntoView?.({ block: 'nearest' }); }, [current]);
  return (
    <div className="files-diff-hunks" data-testid="files-diff-hunks">
      {changes.hunks.map((hunk, index) => {
        const state = states[index];
        const print = state?.fingerprint ?? String(index);
        const handled = isHandled(state);
        const shown = !handled || opened.has(print);
        const toggle = () => {
          onCurrent(index);
          if (handled) setOpened((now) => { const next = new Set(now); if (next.has(print)) next.delete(print); else next.add(print); return next; });
        };
        return (
          <section key={print} ref={(element) => { heads.current[index] = element; }} data-testid={'files-diff-hunk-' + index}
            className={'files-diff-hunk' + (handled ? ' handled' : '') + (index === current ? ' current' : '')}>
            <div className="files-diff-hunk-head" onClick={toggle}>
              <span className="files-diff-hunk-place">{hunkHeader(hunk)}</span>
              {handled && <span className="files-diff-hunk-state">{t('hunks.accepted')}</span>}
              {state?.refusal && <span className="files-diff-hunk-refusal" title={t(RefusalText[state.refusal])}>{t(RefusalChip[state.refusal])}</span>}
              <button type="button" className="btn small" data-testid={'hunk-accept-' + index} disabled={busy || !canAct(state, 'accept')}
                title={t('tooltip.hunks.accept')} onClick={(event) => { event.stopPropagation(); onCurrent(index); onAct('accept', index); }}>{t('hunks.accept')}</button>
              <button type="button" className="btn small" data-testid={'hunk-discard-' + index} disabled={busy || !canAct(state, 'discard')}
                title={state?.refusal ? t(RefusalText[state.refusal]) : t('tooltip.hunks.discard')}
                onClick={(event) => { event.stopPropagation(); onCurrent(index); onAct('discard', index); }}>{t('hunks.discard')}</button>
            </div>
            {shown && (
              <PatchDiff<number> patch={patches[index] ?? ''} options={options} lineAnnotations={annotations} selectedLines={selected} renderAnnotation={renderNote} />
            )}
          </section>
        );
      })}
    </div>
  );
}

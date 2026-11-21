import { lazy, memo, Suspense, useEffect, useMemo, useState } from 'react';
import { Icon } from '../icons/Icon';
import { useServices } from '../app/context';
import { editorClick } from '../conversation/referenceClick';
import { useAppTheme } from '../panes/files/useAppTheme';
import type { CheckpointFile } from '../store/models';
import type { MightyFile } from './types';
import type { BlockActions } from './Block';
import { byFile, foldered, matches, openAtFirst, SEARCH_FROM, SECTION_ROWS, type RollUpSection } from './rollup';

const LazyPatchDiff = lazy(() => import('@pierre/diffs/react').then((m) => ({ default: m.PatchDiff })));

type Translate = ReturnType<typeof useServices>['t'];

/** A section's heading: the step's words, the background task's, or the changes outside any step. */
export function sectionTitle(t: Translate, section: RollUpSection, alone: boolean): string {
  if (section.kind === 'step' && section.label) return section.label;
  if (section.kind === 'turn') return section.label ? t('graph.rollUp.background', { name: section.label }) : t('graph.rollUp.backgroundPlain');
  return alone ? t('graph.rollUp.whole') : t('graph.rollUp.rest');
}

function statusWord(t: Translate, file: { status: string; oldPath?: string }): string {
  switch (file.status) {
    case 'A': return t('graph.rollUp.added');
    case 'D': return t('graph.rollUp.deleted');
    case 'R': return t('graph.rollUp.renamed', { from: file.oldPath ?? '' });
    case 'T': return t('graph.rollUp.typeChanged');
    default: return t('graph.rollUp.modified');
  }
}

const timeOf = (at?: string) => {
  if (!at) return '';
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
};

/**
 * A request's 결과물 beside its last result: what each step, background turn and the rest of the run changed (the snapshots of
 * checkpoints.rs, every agent), then the files the answer names. 단계별 shows a section per part, the older ones folded when there are
 * many and each cut at five rows; 파일별 shows each file once with the steps that changed it. A search box comes past twenty files. A
 * file opens in the preview (its 변경사항), in the editor with ⌘/Ctrl, and its +/− shows what that step changed in it, read from the
 * snapshots.
 */
export const ResultRollUp = memo(function ResultRollUp({ paneId, runId, sections, mentioned, actions }: {
  paneId: string;
  runId: string | null;
  sections: readonly RollUpSection[];
  mentioned: readonly MightyFile[];
  actions: BlockActions;
}) {
  const { t } = useServices();
  const [mode, setMode] = useState<'steps' | 'files'>('steps');
  const [query, setQuery] = useState('');
  const [opened, setOpened] = useState<Set<number>>(() => openAtFirst(sections.length));
  const [grown, setGrown] = useState<Set<string>>(() => new Set());
  // A section added later (the request went on) opens as the newest does.
  useEffect(() => setOpened(openAtFirst(sections.length)), [sections.length]);
  const files = useMemo(() => byFile(sections), [sections]);
  const total = files.length + mentioned.length;
  const searching = total > SEARCH_FROM && query.trim() !== '';
  const open = (file: MightyFile, event: React.MouseEvent) => (editorClick(event) && actions.openInEditor ? actions.openInEditor(file) : actions.openFile(file, runId));
  const toggle = (index: number) => setOpened((now) => { const next = new Set(now); if (next.has(index)) next.delete(index); else next.add(index); return next; });
  const grow = (key: string) => setGrown((now) => new Set(now).add(key));
  const alone = sections.length === 1;
  return (
    <div className="mighty-body mighty-rollup" data-testid="mighty-rollup">
      {sections.length > 0 && (
        <div className="mighty-rollup-bar">
          <span className="mighty-rollup-mode" role="group">
            <button type="button" className={mode === 'steps' ? 'chosen' : ''} aria-pressed={mode === 'steps'} onClick={() => setMode('steps')}>{t('graph.rollUp.bySteps')}</button>
            <button type="button" className={mode === 'files' ? 'chosen' : ''} aria-pressed={mode === 'files'} onClick={() => setMode('files')}>{t('graph.rollUp.byFiles')}</button>
          </span>
          {total > SEARCH_FROM && (
            <input className="mighty-rollup-search" type="search" value={query} placeholder={t('graph.rollUp.search')} aria-label={t('graph.rollUp.search')}
              onChange={(event) => setQuery(event.target.value)} />
          )}
        </div>
      )}
      {mode === 'steps' && sections.map((section, index) => {
        const shown = searching ? section.files.filter((file) => matches(file.path, query)) : section.files;
        if (searching && shown.length === 0) return null;
        const isOpen = searching || opened.has(index);
        const limit = searching || grown.has(section.key) ? shown.length : SECTION_ROWS;
        return (
          <section key={section.key} className="mighty-rollup-section" data-testid="mighty-rollup-section">
            <button type="button" className="mighty-rollup-head" aria-expanded={isOpen} onClick={() => toggle(index)}>
              <Icon name={isOpen ? 'chevron-down' : 'chevron-right'} size={12} weight={2} />
              <span className="mighty-rollup-index">{index + 1}</span>
              <span className="mighty-rollup-title" title={sectionTitle(t, section, alone)}>{sectionTitle(t, section, alone)}</span>
              <span className="mighty-rollup-meta">{t('graph.rollUp.sectionCount', { count: section.files.length + section.more })}{section.at ? ' · ' + timeOf(section.at) : ''}</span>
            </button>
            {isOpen && (
              <div className="mighty-rollup-rows">
                <Rows files={shown.slice(0, limit)} section={section} paneId={paneId} t={t} onOpen={open} />
                {shown.length > limit && (
                  <button type="button" className="mighty-rollup-more" onClick={() => grow(section.key)}>{t('graph.rollUp.more', { count: shown.length - limit })}</button>
                )}
                {section.more > 0 && !searching && <div className="mighty-rollup-note">{t('graph.rollUp.unlisted', { count: section.more })}</div>}
              </div>
            )}
          </section>
        );
      })}
      {mode === 'files' && (
        <div className="mighty-rollup-rows">
          {files.filter((file) => !searching || matches(file.path, query)).map((file) => (
            <div key={file.path} className="mighty-file-row">
              <button className="mighty-file" title={statusWord(t, file) + ' · ' + t('graph.resultFiles.openFile', { path: file.path })} onClick={(event) => open({ path: file.path }, event)}>
                <StatusMark status={file.status} />
                <span className="mighty-file-words">
                  <span className="mighty-file-name">{file.path.split('/').pop()}</span>
                  <span className="mighty-file-path">{file.path}</span>
                </span>
                <Counts added={file.added} removed={file.removed} t={t} />
              </button>
              <span className="mighty-rollup-tags" title={t('graph.rollUp.inSteps', { list: file.sections.map((s) => sectionTitle(t, sections[s]!, alone)).join(', ') })}>
                {file.sections.map((s) => <span key={s} className="mighty-rollup-tag">{s + 1}</span>)}
              </span>
            </div>
          ))}
        </div>
      )}
      {searching && files.every((file) => !matches(file.path, query)) && mentioned.every((file) => !matches(file.path, query)) && (
        <div className="mighty-rollup-note">{t('graph.rollUp.noMatch')}</div>
      )}
      {mentioned.length > 0 && (
        <section className="mighty-rollup-section">
          {sections.length > 0 && <div className="mighty-rollup-head plain"><span className="mighty-rollup-title">{t('graph.rollUp.mentioned')}</span></div>}
          <div className="mighty-rollup-rows">
            {mentioned.filter((file) => !searching || matches(file.path, query)).map((file) => (
              // M/MightyGraphResultFiles.swift: a row per file, its glyph, its name over its path in mono.
              <div key={file.path + ':' + (file.line ?? '')} className="mighty-file-row">
                <button className="mighty-file" title={t('graph.resultFiles.openFile', { path: file.path })} onClick={(event) => open(file, event)}>
                  <Icon name="file-text" size={13} weight={1.6} className="mighty-file-glyph" />
                  <span className="mighty-file-words">
                    <span className="mighty-file-name">{file.path.split(/[\\/]/).pop()}{file.line ? ':' + file.line : ''}</span>
                    <span className="mighty-file-path">{file.path}</span>
                  </span>
                </button>
                {actions.openInEditor && (
                  <button className="mighty-file-editor" data-testid="mighty-file-editor" title={file.line ? t('editor.openLine', { line: file.line }) : t('editor.open')}
                    aria-label={file.line ? t('editor.openLine', { line: file.line }) : t('editor.open')} onClick={() => actions.openInEditor?.(file)}>
                    <Icon name="file-code" size={13} weight={1.7} />
                  </button>
                )}
              </div>
            ))}
          </div>
        </section>
      )}
    </div>
  );
});

function StatusMark({ status }: { status: string }) {
  return <span className={'mighty-rollup-status s-' + status.toLowerCase()} aria-hidden="true">{status}</span>;
}

function Counts({ added, removed, t }: { added?: number; removed?: number; t: Translate }) {
  if (added === undefined || removed === undefined) return <span className="mighty-rollup-counts">{t('graph.rollUp.binary')}</span>;
  return (
    <span className="mighty-rollup-counts">
      {added > 0 && <span className="added">+{added}</span>}
      {removed > 0 && <span className="removed">−{removed}</span>}
    </span>
  );
}

/** A section's rows, under folder headings where three or more share a folder; each row can show what the step changed in it. */
function Rows({ files, section, paneId, t, onOpen }: {
  files: readonly CheckpointFile[];
  section: RollUpSection;
  paneId: string;
  t: Translate;
  onOpen: (file: MightyFile, event: React.MouseEvent) => void;
}) {
  const [showing, setShowing] = useState<string | null>(null);
  return (
    <>
      {foldered(files).map((row) => 'folder' in row
        ? <div key={'folder:' + row.folder} className="mighty-rollup-folder"><Icon name="folder" size={12} weight={1.7} />{row.folder}/</div>
        : (
          <div key={row.file.path} className="mighty-rollup-file">
            <div className="mighty-file-row">
              <button className="mighty-file" title={statusWord(t, row.file) + ' · ' + t('graph.resultFiles.openFile', { path: row.file.path })}
                onClick={(event) => onOpen({ path: row.file.path }, event)}>
                <StatusMark status={row.file.status} />
                <span className="mighty-file-words">
                  <span className="mighty-file-name">{row.file.path.split('/').pop()}</span>
                  <span className="mighty-file-path">{row.file.status === 'R' && row.file.oldPath ? row.file.oldPath + ' → ' + row.file.path : row.file.path}</span>
                </span>
              </button>
              <button className={'mighty-rollup-diff' + (showing === row.file.path ? ' on' : '')} data-testid="mighty-rollup-diff"
                title={t(showing === row.file.path ? 'graph.rollUp.hideDiff' : 'graph.rollUp.showDiff')} aria-label={t(showing === row.file.path ? 'graph.rollUp.hideDiff' : 'graph.rollUp.showDiff')}
                aria-pressed={showing === row.file.path} onClick={() => setShowing(showing === row.file.path ? null : row.file.path)}>
                <Counts added={row.file.added} removed={row.file.removed} t={t} />
              </button>
            </div>
            {showing === row.file.path && <StepDiff paneId={paneId} section={section} file={row.file} t={t} />}
          </div>
        ))}
    </>
  );
}

/** What one step changed in one file: the snapshots' diff, read from the host when opened. */
function StepDiff({ paneId, section, file, t }: { paneId: string; section: RollUpSection; file: CheckpointFile; t: Translate }) {
  const { bridge } = useServices();
  const theme = useAppTheme();
  const [patch, setPatch] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    let live = true;
    bridge.call<string | null>('mightyCheckpointDiff', { paneId, base: section.base, tree: section.tree, path: file.path, oldPath: file.oldPath })
      .then((text) => { if (live) setPatch(text && text.trim() !== '' ? text : null); })
      .catch(() => { if (live) setPatch(null); });
    return () => { live = false; };
  }, [bridge, paneId, section.base, section.tree, file.path, file.oldPath]);
  const options = useMemo(() => ({
    theme: { dark: 'pierre-dark', light: 'pierre-light' },
    themeType: theme,
    diffStyle: 'unified' as const,
    lineDiffType: 'word-alt' as const,
    hunkSeparators: 'line-info-basic' as const,
    overflow: 'scroll' as const,
    disableFileHeader: true,
  }), [theme]);
  if (patch === undefined) return <div className="mighty-rollup-note">{t('graph.rollUp.loading')}</div>;
  if (patch === null) return <div className="mighty-rollup-note">{t('graph.rollUp.diffFailed')}</div>;
  return (
    <div className="mighty-rollup-patch" data-testid="mighty-rollup-patch">
      <Suspense fallback={<div className="mighty-rollup-note">{t('graph.rollUp.loading')}</div>}>
        <LazyPatchDiff patch={patch} options={options} />
      </Suspense>
    </div>
  );
}

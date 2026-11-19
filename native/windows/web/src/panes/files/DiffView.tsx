import { memo, useMemo } from 'react';
import { useServices } from '../../app/context';
import { baseCaption, diffRows, gutterDigits, type DiffRow } from './diff';
import type { FileChanges } from './types';

/**
 * The 변경사항 view of a source preview (M/FileDiffView.swift, the phone's DiffView): a caption naming the base and the
 * counts, the truncated note, then a unified diff in the source's monospace: per hunk its `@@` head, then its lines with
 * the old and new line numbers in two gutters, added lines on the done wash and removed ones on the error wash. A screen
 * reader hears each added or removed line's kind before its text. Memoised on the change set: a bubble resized by its grip
 * re-renders with the same object and leaves the rows alone.
 */
export const DiffView = memo(function DiffView({ changes, onLine, noted, picked }: {
  changes: FileChanges;
  /** 요청 변경 검토: a click on a line (not a hunk's head) picks it for a note; the rows' indexes are `diffRows`'. */
  onLine?: (row: DiffRow, index: number) => void;
  /** Rows that carry a note. */
  noted?: ReadonlySet<number>;
  /** The row a note is being written on. */
  picked?: number | null;
}) {
  const { t } = useServices();
  const rows = useMemo(() => diffRows(changes), [changes]);
  const width = gutterDigits(rows) + 'ch';
  const base = baseCaption(changes);
  const caption = base === 'created' ? t('files.changes.created')
    : base === 'run' ? t('files.changes.baseRun')
    : base === 'head' ? t('files.changes.baseHead', { sha: changes.headSha ?? '' })
    : t('files.changes.baseHeadPlain');
  return (
    <div className="files-diff" data-testid="files-diff">
      <div className="files-diff-caption">
        <span>{caption}</span>
        <span className="files-diff-count added">+{changes.added}</span>
        <span className="files-diff-count removed">−{changes.removed}</span>
      </div>
      {changes.truncated && <div className="files-banner">{t('files.changes.truncated')}</div>}
      <div className="files-diff-rows">
        {rows.map((row, index) => (
          <div key={index} className={'files-diff-row ' + row.kind + (noted?.has(index) ? ' noted' : '') + (picked === index ? ' picked' : '')
            + (onLine && row.kind !== 'hunk' && row.kind !== 'noNewline' ? ' notable' : '')}
            onClick={onLine && row.kind !== 'hunk' && row.kind !== 'noNewline' ? () => onLine(row, index) : undefined}>
            <span className="files-diff-number" style={{ width }}>{row.old ?? ''}</span>
            <span className="files-diff-number" style={{ width }}>{row.new ?? ''}</span>
            <span className="files-diff-op" aria-hidden="true">{row.kind === 'added' ? '+' : row.kind === 'removed' ? '−' : ''}</span>
            {(row.kind === 'added' || row.kind === 'removed') && <span className="visually-hidden">{row.kind === 'added' ? t('files.changes.added') : t('files.changes.removed')} </span>}
            <span className="files-diff-text">{row.kind === 'noNewline' ? '\\ ' + t('files.changes.noNewline') : row.text}</span>
          </div>
        ))}
      </div>
    </div>
  );
});

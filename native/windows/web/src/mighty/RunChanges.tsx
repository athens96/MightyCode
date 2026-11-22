import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { DiffView } from '../panes/files/DiffView';
import { diffRows, editorLine, type DiffRow } from '../panes/files/diff';
import { progressText, RefusalText, reviewable, useHunkReview, type HunkAction, type HunkOutcome, type HunkReview } from '../panes/files/hunkReview';
import type { FileChanges } from '../panes/files/types';
import { usePageOverlay } from '../shell/overlay';
import {
  noteTarget, notePlace, notedRows, revertible, reviewPrompt, reviewedCount, unreviewedPaths,
  type ReviewNote, type RunChangesItem, type RunChangesOutcome, type RunChangesPlan, type RunChangesReverted,
} from './runChangesModel';
import './runChanges.css';

/** `runChangesHunk`'s answer: a discard's outcome, the file's review now and the request's items. */
interface HunkAnswer { outcome?: HunkOutcome; review?: HunkReview | null; items?: RunChangesItem[] }

export interface RunChangesProps {
  paneId: string;
  /** The graph run whose request is reviewed (the host finds the run its originals were kept under). */
  runId: string;
  onClose(): void;
}

/**
 * 요청 변경 검토 (M/RunChangesSheet.swift): the files one request changed on the left, the selected file's diff against its content
 * before the request on the right (`referenceChanges` drawn by the 변경사항 view). A click on a diff line opens a note on it; the notes go
 * to the pane's composer as one follow-up request (`reviewPrompt`), after what it holds. The checked files can be put back
 * (`runChangesRevert`) once confirmed. The popup standard's panel (kind C), over the whole window like the plan document.
 */
export function RunChanges(props: RunChangesProps) {
  return createPortal(<RunChangesDialog {...props} />, document.body);
}

export function RunChangesDialog({ paneId, runId, onClose }: RunChangesProps) {
  const { bridge, store, t } = useServices();
  usePageOverlay(true);
  const [plan, setPlan] = useState<RunChangesPlan | null>(null);
  const [chosen, setChosen] = useState<ReadonlySet<string>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const [working, setWorking] = useState(false);
  const [outcomes, setOutcomes] = useState<ReadonlyMap<string, RunChangesOutcome>>(new Map());
  /** Counts the reverts done, so the shown diff is read again after one. */
  const [revision, setRevision] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [changes, setChanges] = useState<FileChanges | null>(null);
  /** 바뀐 부분별 받기·버리기: the selected file's hunks, a hunk action running, and the last refusal's words. */
  const [review, setReview] = useState<HunkReview | null>(null);
  const [hunkBusy, setHunkBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [loadingDiff, setLoadingDiff] = useState(false);
  const [notes, setNotes] = useState<ReviewNote[]>([]);
  const [picked, setPicked] = useState<{ row: DiffRow; index: number; path: string } | null>(null);
  const [noteText, setNoteText] = useState('');
  const noteField = useRef<HTMLInputElement | null>(null);
  const nextNote = useRef(1);

  const items = plan?.items;
  const canRevert = useMemo(() => revertible(items ?? []), [items]);
  const chosenItems = useMemo(() => canRevert.filter((item) => chosen.has(item.path)), [canRevert, chosen]);

  /** The review as the host answers it now: the choice keeps only what can still be put back, the selection a file still listed. */
  const take = useCallback((next: RunChangesPlan) => {
    setPlan(next);
    const open = new Set(revertible(next.items).map((item) => item.path));
    setChosen((now) => new Set([...now].filter((path) => open.has(path))));
    setSelected((now) => (now !== null && next.items.some((item) => item.path === now) ? now : next.items[0]?.path ?? null));
  }, []);

  useEffect(() => {
    let live = true;
    bridge.call<RunChangesPlan | null>('runChanges', { paneId, runId }).then(
      (value) => { if (live) take(value ?? { root: '', items: [] }); },
      () => { if (live) take({ root: '', items: [] }); });
    return () => { live = false; };
  }, [bridge, paneId, runId, take]);

  useEffect(() => {
    setChanges(null); setReview(null); setPicked(null); setMessage(null);
    if (selected === null) { setLoadingDiff(false); return undefined; }
    let live = true;
    setLoadingDiff(true);
    bridge.call<HunkReview | null>('runChangesHunks', { paneId, path: selected, runId }).then(
      (value) => { if (live) { setReview(value ?? null); setChanges(value?.changes ?? null); setLoadingDiff(false); } },
      () => { if (live) setLoadingDiff(false); });
    return () => { live = false; };
  }, [bridge, paneId, runId, selected, revision]);

  /** A hunk answer applied: the file's review as it is now, the request's items with their progress. */
  const takeHunks = useCallback((answer: HunkAnswer) => {
    if (answer.review !== undefined) { setReview(answer.review); setChanges(answer.review?.changes ?? null); setPicked(null); }
    if (answer.items) take({ root: plan?.root ?? '', items: answer.items });
    const outcome = answer.outcome;
    if (outcome?.kind === 'refused') setMessage(t(RefusalText[outcome.refusal]));
    else if (outcome?.kind === 'failed') setMessage(t('runChanges.outcome.failed', { error: outcome.error }));
    else setMessage(null);
  }, [plan?.root, take, t]);

  /** 받기 ✓ (only recorded) or 버리기 ↶ (the hunk's lines put back) of one hunk of the selected file. */
  const actOnHunk = useCallback((action: HunkAction, index: number) => {
    const state = review?.hunks[index];
    const hunk = review?.changes?.hunks[index];
    if (selected === null || !state || !hunk || hunkBusy) return;
    setHunkBusy(true);
    void bridge.call<HunkAnswer | null>('runChangesHunk', {
      paneId, runId, path: selected, action, fingerprints: [state.fingerprint], ...(action === 'discard' ? { hunk } : {}),
    }).then((answer) => { if (answer) takeHunks(answer); }, () => {}).finally(() => setHunkBusy(false));
  }, [bridge, paneId, runId, selected, review, hunkBusy, takeHunks]);

  /** 남은 부분 모두 받기: every hunk not handled yet in every file. */
  const acceptRest = () => {
    const paths = unreviewedPaths(items ?? []);
    if (paths.length === 0 || hunkBusy) return;
    setHunkBusy(true);
    void bridge.call<HunkAnswer | null>('runChangesHunk', { paneId, runId, action: 'acceptAll', paths, ...(selected !== null ? { path: selected } : {}) })
      .then((answer) => { if (answer) takeHunks(answer); }, () => {}).finally(() => setHunkBusy(false));
  };

  const { current: currentHunk, setCurrent: setCurrentHunk, onKeyDown: hunkKeys } = useHunkReview(selected, review, actOnHunk);

  useEffect(() => { if (picked) noteField.current?.focus(); }, [picked]);

  const rows = useMemo(() => (changes ? diffRows(changes) : []), [changes]);
  /** 편집기에서 열기: the selected file at `line` (the host resolves it in the pane's folder again, editor.rs). */
  const openInEditor = useCallback((line: number | null) => {
    if (selected === null) return;
    void bridge.call('openInEditor', { source: 'reference', paneId, path: selected, ...(line ? { line } : {}) }).catch(() => {});
  }, [bridge, paneId, selected]);
  const onOpenLine = useCallback((_row: DiffRow, index: number) => {
    openInEditor(editorLine(rows, index));
  }, [openInEditor, rows]);
  const onLine = useCallback((row: DiffRow, index: number) => {
    if (selected && noteTarget(row)) { setPicked({ row, index, path: selected }); setNoteText(''); }
  }, [selected]);
  /** The header's button: the line a note is being written on, else the first changed line. */
  const firstChange = rows.findIndex((row) => row.kind === 'added' || row.kind === 'removed');
  const editorAt = picked?.path === selected ? editorLine(rows, picked.index) : firstChange >= 0 ? editorLine(rows, firstChange) : null;
  const noted = useMemo(() => (selected ? notedRows(rows, notes, selected) : new Set<number>()), [rows, notes, selected]);

  const revert = async () => {
    setConfirming(false);
    const paths = chosenItems.map((item) => item.path);
    if (paths.length === 0) return;
    setWorking(true);
    try {
      const answer = await bridge.call<RunChangesReverted | null>('runChangesRevert', { paneId, runId, paths });
      if (answer) {
        setOutcomes((now) => { const next = new Map(now); for (const outcome of answer.outcomes) next.set(outcome.path, outcome); return next; });
        take({ root: plan?.root ?? '', items: answer.items });
      }
    } catch { /* the review stays as it was; nothing it shows changed */ }
    setRevision((n) => n + 1);
    setWorking(false);
  };

  const addNote = () => {
    const text = noteText.trim();
    const target = picked ? noteTarget(picked.row) : null;
    if (!picked || !target || text === '') return;
    setNotes((now) => [...now, { id: nextNote.current++, path: picked.path, line: target.line, side: target.side, quote: picked.row.text, text }]);
    setPicked(null); setNoteText('');
  };

  const sendNotes = () => {
    const prompt = reviewPrompt(notes, t('runChanges.notes.header'), t('runChanges.notes.removedLine'));
    if (prompt === '') return;
    store.dispatch({ kind: 'fill', paneId, text: prompt, append: true });
    onClose();
  };

  const detail = (item: RunChangesItem): string => {
    const outcome = outcomes.get(item.path);
    if (outcome?.kind === 'reverted') return t('runChanges.outcome.reverted');
    if (outcome?.kind === 'failed') return t('runChanges.outcome.failed', { error: outcome.error });
    if (item.refusal) return t('runChanges.refusal.' + item.refusal);
    return t(item.action === 'trash' ? 'runChanges.action.trash' : 'runChanges.action.restore');
  };

  const done = [...outcomes.values()].filter((o) => o.kind === 'reverted').length;
  const summary = !items ? '' : message ?? (done > 0 ? t('runChanges.summary.done', { count: done })
    : t('runChanges.summary', { count: items.length, revertible: canRevert.length }) + ' · ' + t('hunks.summary', { reviewed: reviewedCount(items), count: items.length }));
  const rest = unreviewedPaths(items ?? []);
  const allChosen = canRevert.length > 0 && chosenItems.length === canRevert.length;
  const placeOf = picked ? noteTarget(picked.row) : null;
  const stop = (event: { stopPropagation(): void }) => event.stopPropagation();
  const key = (event: KeyboardEvent) => event.stopPropagation();
  const panelKey = (event: KeyboardEvent) => { if (!confirming) hunkKeys(event); event.stopPropagation(); };
  const hunkControls = reviewable(review) ? {
    states: review.hunks, current: currentHunk, busy: hunkBusy || working, onCurrent: setCurrentHunk, onAct: actOnHunk,
  } : undefined;

  return (
    <>
      <DialogShell kind="panel" panel="runChanges" width={1000} height={640} minWidth={640} minHeight={360} label={t('runChanges.title')}
        testId="run-changes" icon={<Icon name="file-search" size={18} weight={1.8} />} title={t('runChanges.title')}
        subtitle={plan?.root || undefined} busy={working} top={!confirming} closeTestId="run-changes-close" onCancel={onClose} onKeyDown={panelKey}
        tools={selected !== null ? (
          <button type="button" className="icon-button" data-testid="run-changes-editor" onClick={() => openInEditor(editorAt)}
            aria-label={editorAt ? t('editor.openLine', { line: editorAt }) : t('editor.open')} title={editorAt ? t('editor.openLine', { line: editorAt }) : t('editor.open')}>
            <Icon name="file-code" size={15} weight={1.8} />
          </button>
        ) : undefined}
        bodyClassName="run-changes-body"
        backdrop={{ 'data-page-overlay': '', onPointerDown: stop, onWheel: stop }}
        status={summary}
        buttons={<>
          <button type="button" className="btn" title={t('tooltip.runChanges.revertChosen')} data-testid="run-changes-revert" disabled={chosenItems.length === 0 || working}
            onClick={() => setConfirming(true)}>{t('runChanges.revertChosen', { count: chosenItems.length })}</button>
          <button type="button" className="btn primary" title={t('tooltip.runChanges.sendNotes')} data-testid="run-changes-send-notes" disabled={notes.length === 0 || working}
            onClick={sendNotes}>{t('runChanges.notes.send', { count: notes.length })}</button>
        </>}>
        {!items ? <DialogLoading rows={3} />
          : items.length === 0 ? <div className="run-changes-placeholder" data-testid="run-changes-empty">{t('runChanges.empty')}</div>
          : (
            <div className="run-changes-split">
              <div className="run-changes-list">
                <div className="run-changes-list-head">
                  <span>{t('runChanges.files', { count: items.length })}</span>
                  <button type="button" className="btn text" title={t('tooltip.hunks.acceptRest')} data-testid="run-changes-accept-rest"
                    disabled={rest.length === 0 || hunkBusy || working} onClick={acceptRest}>{t('hunks.acceptRest')}</button>
                  <button type="button" className="btn text" title={t('tooltip.runChanges.toggleAll')} disabled={canRevert.length === 0 || working}
                    onClick={() => setChosen(allChosen ? new Set() : new Set(canRevert.map((item) => item.path)))}>
                    {t(allChosen ? 'runChanges.chooseNone' : 'runChanges.chooseAll')}
                  </button>
                </div>
                <div className="run-changes-rows" role="list">
                  {items.map((item) => (
                    <div key={item.path} role="listitem" data-testid={'run-changes-row-' + item.path}
                      className={'run-changes-row' + (selected === item.path ? ' selected' : '') + (item.refusal ? ' refused' : '')}
                      onClick={() => setSelected(item.path)}>
                      <input type="checkbox" title={t('tooltip.runChanges.checkFile')} aria-label={item.path} checked={chosen.has(item.path)} disabled={item.refusal !== undefined || working}
                        onClick={stop}
                        onChange={(event) => setChosen((now) => { const next = new Set(now); if (event.target.checked) next.add(item.path); else next.delete(item.path); return next; })} />
                      <span className="run-changes-icon" aria-hidden="true"><Icon name={item.action === 'trash' ? 'plus' : 'file-text'} size={13} weight={1.75} /></span>
                      <span className="run-changes-name">
                        <span className="run-changes-path" title={item.path}>{item.path}</span>
                        <span className="run-changes-detail">{detail(item)}</span>
                      </span>
                      {progressText({ handled: item.handled ?? 0, total: item.hunks ?? 0 }) && (
                        <span className={'run-changes-progress' + (item.reviewed ? ' reviewed' : '')} data-testid={'run-changes-progress-' + item.path}
                          title={item.reviewed ? t('hunks.reviewed') : t('hunks.progress', { handled: item.handled ?? 0, total: item.hunks ?? 0 })}>
                          {item.reviewed ? '✓ ' : ''}{progressText({ handled: item.handled ?? 0, total: item.hunks ?? 0 })}
                        </span>
                      )}
                      {(item.restored > 0 || item.removed > 0) && (
                        <span className="run-changes-counts" title={detail(item)}>
                          <span className="added">+{item.restored}</span> <span className="removed">−{item.removed}</span>
                        </span>
                      )}
                    </div>
                  ))}
                </div>
              </div>
              <div className="run-changes-review">
                <div className="run-changes-diff">
                  {changes ? (changes.unchanged ? <div className="run-changes-placeholder">{t('runChanges.diff.unchanged')}</div>
                    : <DiffView changes={changes} name={selected ?? undefined} noted={noted} picked={picked?.path === selected ? picked.index : null}
                      onOpenLine={onOpenLine} onLine={onLine} hunks={hunkControls} />)
                    : loadingDiff ? <DialogLoading rows={2} />
                    : <div className="run-changes-placeholder">{t(selected === null ? 'runChanges.diff.pick' : 'runChanges.diff.none')}</div>}
                </div>
                {picked && placeOf ? (
                  <div className="run-changes-note-editor">
                    <span className="run-changes-note-on">{t('runChanges.notes.on', { place: notePlace(picked.path, placeOf) })}</span>
                    <div className="run-changes-note-line">
                      <input ref={noteField} className="dialog-field" title={t('tooltip.runChanges.noteInput')} data-testid="run-changes-note-field" value={noteText} placeholder={t('runChanges.notes.placeholder')}
                        onChange={(event) => setNoteText(event.target.value)}
                        onKeyDown={(event) => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); addNote(); } }} />
                      <button type="button" className="btn" title={t('tooltip.runChanges.noteAdd')} disabled={noteText.trim() === ''} onClick={addNote}>{t('runChanges.notes.add')}</button>
                      <button type="button" className="btn" onClick={() => { setPicked(null); setNoteText(''); }}>{t('runChanges.confirm.cancel')}</button>
                    </div>
                  </div>
                ) : changes && !changes.unchanged ? <div className="run-changes-hint">{t('runChanges.notes.hint')} {t('editor.lineHint')} {hunkControls ? t('hunks.keysHint') : ''}</div> : null}
                {notes.length > 0 && (
                  <div className="run-changes-notes" data-testid="run-changes-notes">
                    <span className="run-changes-note-on">{t('runChanges.notes.title', { count: notes.length })}</span>
                    <div className="run-changes-notes-list">
                      {notes.map((note) => (
                        <div key={note.id} className="run-changes-note">
                          <button type="button" className="run-changes-note-place" title={t('tooltip.runChanges.noteJump')} onClick={() => setSelected(note.path)}>{notePlace(note.path, note)}</button>
                          <span className="run-changes-note-text">{note.text}</span>
                          <button type="button" className="run-changes-note-remove" aria-label={t('runChanges.notes.remove')} title={t('runChanges.notes.remove')}
                            onClick={() => setNotes((now) => now.filter((n) => n.id !== note.id))}><Icon name="circle-x" size={13} weight={1.75} /></button>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          )}
      </DialogShell>
      {confirming && (
        <DialogShell kind="alert" label={t('runChanges.confirm.title', { count: chosenItems.length })} title={t('runChanges.confirm.title', { count: chosenItems.length })}
          testId="run-changes-confirm" onCancel={() => setConfirming(false)} onKeyDown={key} backdrop={{ onPointerDown: stop, onWheel: stop }}
          buttons={<>
            <button type="button" className="btn" data-autofocus onClick={() => setConfirming(false)}>{t('runChanges.confirm.cancel')}</button>
            <button type="button" className="btn danger" data-testid="run-changes-confirm-revert" onClick={() => void revert()}>{t('runChanges.confirm.revert')}</button>
          </>}>
          <p className="dialog-message">{t('runChanges.confirm.message')}</p>
        </DialogShell>
      )}
    </>
  );
}

// The dialogs the window's flows ask the page to draw (MainWindow.WebShell.Shell.cs OpenWebDialog): a confirmation (remove
// workspace, delegation, parent close), the rename sheet (spec section 9), the resume sheets (section 3), the two create
// sheets (WorkspaceCreate.tsx) and 대화 검색 (ConversationSearch.tsx). Each answers with `dialogAnswer`; what the answer does stays the host's flow. All of them are
// the popup standard's alert (A) or form sheet (B) (DialogShell.tsx).
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { useServices, useStoreSelector } from '../app/context';
import { ProviderMark } from '../conversation/ProviderMark';
import { BetaBadge, MiddlePath } from '../shell/parts';
import { Icon } from '../icons/Icon';
import type { DialogRequest, OpenDialog, ResumeRows } from '../shell/types';
import { GitCreate, NewFolder } from './WorkspaceCreate';
import { ConversationSearch } from './ConversationSearch';
import { Pending } from '../pending/Pending';
import { DialogLoading, DialogShell } from './DialogShell';
import { diffLines, newSideLines } from '../pane/worktree';
import { diffRows, editorLine } from '../panes/files/diff';
import {
  progressText, RefusalText, reviewable, useHunkReview, type HunkAction, type HunkOutcome, type HunkProgress, type HunkReview,
} from '../panes/files/hunkReview';
import '../pane/worktree.css';

const LazyDiffView = lazy(() => import('../panes/files/DiffView').then((m) => ({ default: m.DiffView })));

/**
 * A sheet over the window (popup standard kind A or B): its title (and subtitle) in the header, its form in the body, its
 * status and buttons in the footer; it focuses its `data-autofocus` control (else its first), and Esc is its cancel.
 */
export function Sheet({ id, width, label, kind = 'sheet', title, subtitle, icon, status, buttons, children, onCancel, onKeyDown }: {
  id: number; width?: number; label: string; kind?: 'alert' | 'sheet'; title?: ReactNode; subtitle?: ReactNode; icon?: ReactNode;
  status?: ReactNode; buttons?: ReactNode; children?: ReactNode; onCancel: () => void; onKeyDown?: (event: KeyboardEvent) => void;
}) {
  return (
    <DialogShell kind={kind} width={width} label={label} title={title ?? label} subtitle={subtitle} icon={icon} status={status} buttons={buttons} onCancel={onCancel}
      onKeyDown={onKeyDown}
      backdrop={{ 'data-testid': 'dialog', 'data-dialog-id': id }}>
      {children}
    </DialogShell>
  );
}

function Confirm({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'confirm' }>; answer: (value: unknown) => void }) {
  return (
    <Sheet id={id} kind="alert" label={dialog.title} onCancel={() => answer(false)}
      buttons={<>
        <button type="button" className="btn" data-autofocus={dialog.defaultPrimary ? undefined : true} onClick={() => answer(false)}>{dialog.close}</button>
        {dialog.primary && (
          <button type="button" className={'btn ' + (dialog.destructive ? 'danger' : 'primary')} data-testid="dialog-primary" data-autofocus={dialog.defaultPrimary ? true : undefined}
            onClick={() => answer(true)}>{dialog.primary}</button>
        )}
      </>}>
      {dialog.message && <p className="dialog-message selectable">{dialog.message}</p>}
    </Sheet>
  );
}

function Rename({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'rename' }>; answer: (value: unknown) => void }) {
  const { bridge } = useServices();
  const [name, setName] = useState(dialog.current);
  const [check, setCheck] = useState<{ valid: boolean; messages: string[] }>({ valid: true, messages: [] });
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => { field.current?.select(); }, []);
  // Every rule is RenameSupport's, asked of the host as the name changes.
  useEffect(() => {
    let live = true;
    void bridge.call<{ valid: boolean; messages: string[] }>('renameCheck', { text: name }).then((value) => { if (live && value) setCheck(value); }, () => {});
    return () => { live = false; };
  }, [bridge, name]);
  const save = () => { if (check.valid) answer({ choice: 'save', name }); };
  return (
    <Sheet id={id} width={360} label={dialog.title} onCancel={() => answer({ choice: 'cancel' })}
      status={dialog.automatic && <button type="button" className="btn text" onClick={() => answer({ choice: 'automatic' })}>{dialog.automatic}</button>}
      buttons={<>
        <button type="button" className="btn" onClick={() => answer({ choice: 'cancel' })}>{dialog.cancel}</button>
        <button type="button" className="btn primary" disabled={!check.valid} onClick={save} data-testid="rename-save">{dialog.save}</button>
      </>}>
      <input ref={field} className="dialog-field" data-autofocus placeholder={dialog.field} aria-label={dialog.field} value={name} data-testid="rename-field"
        onChange={(event) => setName(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); save(); } }} />
      <div className="sheet-caption">{dialog.hint}</div>
      {check.messages.map((message) => <div key={message} className="sheet-caption tone-text-err">{message}</div>)}
    </Sheet>
  );
}

function ResumeChoice({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'resumeChoice' }>; answer: (value: unknown) => void }) {
  const { t } = useServices();
  return (
    <Sheet id={id} width={460} label={dialog.title} icon={<ProviderMark provider={dialog.provider} size={20} />} onCancel={() => answer(null)}
      buttons={<>
        <button type="button" className="btn" onClick={() => answer(null)}>{t('resume.cancel')}</button>
        <button type="button" className="btn" title={t('tooltip.resume.resume')} disabled={dialog.checking} onClick={() => answer('resume')} data-testid="resume-choice-resume">{t('resume.choice.resume')}</button>
        <button type="button" className="btn primary" title={t('tooltip.resume.startNew')} data-autofocus onClick={() => answer('new')} data-testid="resume-choice-new">{t('resume.choice.startNew')}</button>
      </>}>
      {dialog.checking ? <div className="quiet"><Pending busy showsLabel={false} /> {t('resume.loading')}</div> : <p className="dialog-message">{dialog.message}</p>}
    </Sheet>
  );
}

function ResumeList({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'resumeList' }>; answer: (value: unknown) => void }) {
  const { bridge, t } = useServices();
  const [search, setSearch] = useState('');
  const [all, setAll] = useState(false);
  const [rows, setRows] = useState<ResumeRows | null>(null);
  useEffect(() => {
    let live = true;
    void bridge.call<ResumeRows | null>('resumeRows', { search, all }).then((value) => { if (live) setRows(value ?? { rows: [] }); }, () => {});
    return () => { live = false; };
  }, [bridge, search, all]);
  return (
    <Sheet id={id} width={560} label={dialog.title} icon={<ProviderMark provider={dialog.provider} size={16} />} subtitle={<MiddlePath path={dialog.path} className="mono" />}
      onCancel={() => answer(null)} status={<span className="quiet small">{t('resume.note')}</span>}
      buttons={<button type="button" className="btn" onClick={() => answer(null)}>{t('resume.cancel')}</button>}>
      {/* The Mac's 520-high sheet, less its header and footer. */}
      <div className="resume-list" style={{ height: 520 - 48 - 52 - 16 }}>
        {/* M/ResumeSessionViews.swift: the search field with its magnifier inside. */}
        <label className="resume-search">
          <Icon name="search" size={13} weight={2} />
          <input className="dialog-field sheet-field" type="search" data-autofocus placeholder={t('resume.search')} aria-label={t('resume.search')} value={search} onChange={(event) => setSearch(event.target.value)} />
        </label>
        <div className="resume-rows" data-testid="resume-rows">
          {rows === null ? <DialogLoading />
            : rows.rows.length === 0 ? <div className="resume-empty"><strong>{rows.empty}</strong>{rows.reason && <span className="quiet">{rows.reason}</span>}</div>
            : rows.rows.map((row) => (
              <button key={row.id} type="button" className="resume-row" title={row.help} onClick={() => answer(row.id)}>
                <ProviderMark provider={row.provider} size={14} />
                <span className="resume-row-text">
                  <span className={'resume-row-title' + (row.untitled ? ' quiet' : '')}>{row.title}{row.beta && <BetaBadge />}</span>
                  <span className="quiet small">{row.details}</span>
                  {row.busy && <span className="tone-text-wait small">{t('resume.recentlyModified')}</span>}
                </span>
              </button>
            ))}
        </div>
        <div className="resume-footer">
          <label className="small" title={t('tooltip.resume.showAll')}><input type="checkbox" title={t('tooltip.resume.showAll')} checked={all} onChange={(event) => setAll(event.target.checked)} /> {t('resume.showAll')}</label>
          {rows?.hidden && <span className="quiet small">{rows.hidden}</span>}
        </div>
      </div>
    </Sheet>
  );
}

/** Where a new agent pane works (MainWindow.PaneWorktree.cs AskPaneWorktreeChoice): its own worktree, always for this workspace, or the folder. */
function PaneWorktreeChoice({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'paneWorktreeChoice' }>; answer: (value: unknown) => void }) {
  return (
    <Sheet id={id} width={520} label={dialog.title} icon={<Icon name="git-branch" size={18} />} onCancel={() => answer(null)}
      buttons={<>
        <button type="button" className="btn" onClick={() => answer(null)}>{dialog.cancel}</button>
        <button type="button" className="btn" onClick={() => answer('same')} data-testid="pane-worktree-same" data-autofocus={dialog.canWorktree ? undefined : true}>{dialog.sameFolder}</button>
        {dialog.canWorktree && <button type="button" className="btn" onClick={() => answer('always')} data-testid="pane-worktree-always">{dialog.always}</button>}
        {dialog.canWorktree && <button type="button" className="btn primary" data-autofocus onClick={() => answer('worktree')} data-testid="pane-worktree-new">{dialog.worktree}</button>}
      </>}>
      <p className="dialog-message">{dialog.message}</p>
    </Sheet>
  );
}

/** MC-28: 새 worktree로 갈라내기 / 여기서 갈라내기: what happens, and whether the uncommitted changes come along (on by default). */
function PaneFork({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'paneFork' }>; answer: (value: unknown) => void }) {
  const [carry, setCarry] = useState(dialog.canCarry);
  return (
    <Sheet id={id} width={480} label={dialog.title} icon={<Icon name="git-fork" size={18} />} onCancel={() => answer(null)}
      buttons={<>
        <button type="button" className="btn" onClick={() => answer(null)}>{dialog.cancel}</button>
        <button type="button" className="btn primary" data-autofocus onClick={() => answer({ carry })} data-testid="pane-fork-go">{dialog.fork}</button>
      </>}>
      <p className="dialog-message">{dialog.message}</p>
      {dialog.canCarry
        ? <label className="small"><input type="checkbox" checked={carry} onChange={(event) => setCarry(event.target.checked)} data-testid="pane-fork-carry" /> {dialog.carry}</label>
        : <p className="dialog-message small">{dialog.noChanges}</p>}
    </Sheet>
  );
}

/** A one-line worktree setting: the setup command or the files copied into new worktrees. */
function PaneWorktreeText({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'paneWorktreeText' }>; answer: (value: unknown) => void }) {
  const [text, setText] = useState(dialog.current);
  return (
    <Sheet id={id} width={460} label={dialog.title} onCancel={() => answer(null)}
      buttons={<>
        <button type="button" className="btn" onClick={() => answer(null)}>{dialog.cancel}</button>
        <button type="button" className="btn primary" onClick={() => answer(text)} data-testid="pane-worktree-text-save">{dialog.save}</button>
      </>}>
      <p className="dialog-message">{dialog.message}</p>
      <input className="dialog-field" data-autofocus placeholder={dialog.placeholder} aria-label={dialog.title} value={text} data-testid="pane-worktree-text"
        onChange={(event) => setText(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); answer(text); } }} />
    </Sheet>
  );
}

/** `paneWorktreeHunk`'s answer: a discard's outcome and the file's review now. */
interface WorktreeHunkAnswer { outcome?: HunkOutcome; review?: HunkReview | null }

/**
 * The files a worktree pane (or a delegated child, from the review queue, or a compare run's child) changed against its base, and the
 * picked file's diff. Where git can read the base as text, the diff is drawn hunk by hunk with 받기 ✓ / 버리기 ↶ (바뀐 부분별
 * 받기·버리기, `paneWorktreeHunks`); the list shows each file's progress and 남은 부분 모두 받기 takes what is left; `]` `[` `a` `x`
 * while the sheet has focus. Otherwise the plain diff (paneWorktreeDiff). 편집기에서 열기 opens the picked file, a right-click on a diff
 * line opens it at that line (editor.rs resolves the path in the worktree again).
 */
function PaneWorktreeChanges({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'paneWorktreeChanges' }>; answer: (value: unknown) => void }) {
  const { bridge, t } = useServices();
  const [picked, setPicked] = useState<string | null>(dialog.files[0]?.path ?? null);
  const [diff, setDiff] = useState<string | null>(null);
  const [review, setReview] = useState<HunkReview | null>(null);
  const [progress, setProgress] = useState<Record<string, HunkProgress>>({});
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  /** Counts the 남은 부분 모두 받기 done, so the list's progress is read again. */
  const [revision, setRevision] = useState(0);
  const paths = useMemo(() => dialog.files.map((file) => file.path), [dialog.files]);
  useEffect(() => {
    let live = true;
    if (paths.length > 0) void bridge.call<Record<string, HunkProgress> | null>('paneWorktreeReview', { id: dialog.paneId, paths }).then((value) => { if (live) setProgress(value ?? {}); }, () => {});
    return () => { live = false; };
  }, [bridge, dialog.paneId, paths, revision]);
  useEffect(() => {
    let live = true; setDiff(null); setReview(null); setMessage(null);
    if (!picked) return () => { live = false; };
    const plain = () => { void bridge.call<string | null>('paneWorktreeDiff', { id: dialog.paneId, path: picked }).then((value) => { if (live) setDiff(value ?? ''); }, () => {}); };
    void bridge.call<HunkReview | null>('paneWorktreeHunks', { id: dialog.paneId, path: picked }).then((value) => {
      if (!live) return;
      if (value && !value.refusal && value.changes) setReview(value); else plain();
    }, plain);
    return () => { live = false; };
  }, [bridge, dialog.paneId, picked, revision]);
  const editor = (line: number | null) => {
    if (picked) void bridge.call('openInEditor', { source: 'worktree', id: dialog.paneId, path: picked, ...(line ? { line } : {}) }).catch(() => {});
  };
  const take = useCallback((path: string, next: WorktreeHunkAnswer) => {
    if (next.review !== undefined) {
      setReview(next.review);
      const now = next.review;
      if (now && !now.refusal) setProgress((before) => ({ ...before, [path]: { handled: now.handled, total: now.total, reviewed: now.handled === now.total } }));
    }
    const outcome = next.outcome;
    setMessage(outcome?.kind === 'refused' ? t(RefusalText[outcome.refusal]) : outcome?.kind === 'failed' ? t('runChanges.outcome.failed', { error: outcome.error }) : null);
  }, [t]);
  const act = useCallback((action: HunkAction, index: number) => {
    const state = review?.hunks[index];
    const hunk = review?.changes?.hunks[index];
    if (!picked || !state || !hunk || busy) return;
    setBusy(true);
    const path = picked;
    void bridge.call<WorktreeHunkAnswer | null>('paneWorktreeHunk', {
      id: dialog.paneId, path, action, fingerprints: [state.fingerprint], ...(action === 'discard' ? { hunk } : {}),
    }).then((next) => { if (next) take(path, next); }, () => {}).finally(() => setBusy(false));
  }, [bridge, dialog.paneId, picked, review, busy, take]);
  const rest = dialog.files.filter((file) => { const p = progress[file.path]; return p && !p.reviewed; }).map((file) => file.path);
  const acceptRest = () => {
    if (rest.length === 0 || busy) return;
    setBusy(true);
    void bridge.call('paneWorktreeHunk', { id: dialog.paneId, action: 'acceptAll', paths: rest }).catch(() => {})
      .finally(() => { setBusy(false); setRevision((n) => n + 1); });
  };
  const { current, setCurrent, onKeyDown } = useHunkReview(picked, review, act);
  const lines = diff === null ? [] : newSideLines(diff);
  const rows = useMemo(() => (review?.changes ? diffRows(review.changes) : []), [review]);
  const controls = reviewable(review) ? { states: review.hunks, current, busy, onCurrent: setCurrent, onAct: act } : undefined;
  const status = message ?? (picked && (diff || controls) ? t('editor.lineHint') + (controls ? ' ' + t('hunks.keysHint') : '') : undefined);
  return (
    <Sheet id={id} width={880} label={dialog.title} onCancel={() => answer(null)} status={status} onKeyDown={onKeyDown}
      buttons={<>
        {rest.length > 0 && <button type="button" className="btn" data-testid="worktree-changes-accept-rest" title={t('tooltip.hunks.acceptRest')} disabled={busy} onClick={acceptRest}>{t('hunks.acceptRest')}</button>}
        {picked && dialog.paneId && <button type="button" className="btn" data-testid="worktree-changes-editor" onClick={() => editor(null)}>{t('editor.open')}</button>}
        <button type="button" className="btn" data-autofocus onClick={() => answer(null)}>{t('dialog.close')}</button>
      </>}>
      {dialog.files.length === 0 ? <p className="dialog-message">{dialog.none}</p> : (
        <div className="worktree-changes">
          <ul>{dialog.files.map((file) => {
            const shown = progressText(progress[file.path]);
            return (
              <li key={file.path}><button type="button" className={(file.path === picked ? 'active' : '') + (progress[file.path]?.reviewed ? ' reviewed' : '')} title={file.path} onClick={() => setPicked(file.path)}>
                <span className="worktree-changes-name">{file.status + '  ' + file.path}</span>
                {shown && <span className="worktree-changes-progress" title={progress[file.path]?.reviewed ? t('hunks.reviewed') : t('hunks.progress', { handled: progress[file.path]!.handled, total: progress[file.path]!.total })}>
                  {progress[file.path]?.reviewed ? '✓ ' : ''}{shown}</span>}
              </button></li>
            );
          })}</ul>
          {!picked ? <p className="quiet">{dialog.pick}</p>
            : review?.changes ? (review.changes.unchanged ? <p className="quiet">{t('hunks.sameAsBase')}</p> : (
              <div className="worktree-hunks">
                <Suspense fallback={<Pending busy showsLabel={false} />}>
                  <LazyDiffView changes={review.changes} name={picked} hunks={controls} onOpenLine={(_row, index) => editor(editorLine(rows, index))} />
                </Suspense>
              </div>
            ))
            : diff === null ? <Pending busy showsLabel={false} /> : (
              <pre className="worktree-diff selectable">{diffLines(diff).map((line, index) => (
                <div key={index} className={line.kind} title={lines[index] ? t('editor.lineHint') : undefined} onContextMenu={lines[index] ? (event) => { event.preventDefault(); editor(lines[index] ?? null); } : undefined}>{line.text || ' '}</div>
              ))}</pre>
            )}
        </div>
      )}
    </Sheet>
  );
}

/** A workspace's pane worktrees: open, kept and found ones, each with its actions (MainWindow.PaneWorktree.cs ShowWorktreeList). */
function PaneWorktreeList({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'paneWorktreeList' }>; answer: (value: unknown) => void }) {
  return (
    <Sheet id={id} width={640} label={dialog.title} onCancel={() => answer(null)} buttons={<button type="button" className="btn" data-autofocus onClick={() => answer(null)}>{dialog.close}</button>}>
      {dialog.rows.length === 0 ? <p className="dialog-message">{dialog.empty}</p> : dialog.rows.map((row) => (
        <div key={row.id} className="worktree-list-row" data-testid={'worktree-row-' + row.id}>
          <Icon name="git-branch" size={14} />
          <div className="text">
            <span><strong>{row.branch}</strong> · {row.originText}</span>
            <span className="quiet small">{row.counts}</span>
            <span className="quiet small" title={row.path}>{row.path}</span>
          </div>
          {row.actions.map((item) => (
            <button key={item.action} type="button" className={'btn small' + (item.action === 'delete' ? ' danger' : '')} onClick={() => answer({ id: row.id, action: item.action })}>{item.label}</button>
          ))}
        </div>
      ))}
    </Sheet>
  );
}

function DialogView({ open }: { open: OpenDialog }) {
  const { bridge, store } = useServices();
  const answer = (value: unknown) => {
    store.dispatch({ kind: 'dialogClose', id: open.id });
    void bridge.call('dialogAnswer', { id: open.id, answer: value }).catch(() => {});
  };
  const { dialog } = open;
  switch (dialog.type) {
    case 'confirm': return <Confirm id={open.id} dialog={dialog} answer={answer} />;
    case 'rename': return <Rename id={open.id} dialog={dialog} answer={answer} />;
    case 'resumeChoice': return <ResumeChoice id={open.id} dialog={dialog} answer={answer} />;
    case 'resumeList': return <ResumeList id={open.id} dialog={dialog} answer={answer} />;
    case 'conversationSearch': return <ConversationSearch id={open.id} dialog={dialog} answer={answer} />;
    case 'newFolder': return <NewFolder id={open.id} dialog={dialog} answer={answer} />;
    case 'gitCreate': return <GitCreate id={open.id} dialog={dialog} answer={answer} />;
    case 'paneWorktreeChoice': return <PaneWorktreeChoice id={open.id} dialog={dialog} answer={answer} />;
    case 'paneWorktreeText': return <PaneWorktreeText id={open.id} dialog={dialog} answer={answer} />;
    case 'paneWorktreeChanges': return <PaneWorktreeChanges id={open.id} dialog={dialog} answer={answer} />;
    case 'paneWorktreeList': return <PaneWorktreeList id={open.id} dialog={dialog} answer={answer} />;
    case 'paneFork': return <PaneFork id={open.id} dialog={dialog} answer={answer} />;
  }
}

/** The newest dialog over the window; the others wait under it. */
export function Dialogs() {
  const dialogs = useStoreSelector((state) => state.dialogs);
  const top = dialogs[dialogs.length - 1];
  return top ? <DialogView key={top.id} open={top} /> : null;
}

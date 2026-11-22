// The Checks tab of the Git workbench (MC-27; checksModel.ts, src-tauri merge_checks.rs): the uncommitted changes and distance from the base,
// the pull request, each CI check, the unresolved review threads and the remaining checklist items of the pane's last request, each
// on its own (a part that cannot be read says so). Read when the tab opens and every 30 s while it stays open; never otherwise.
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useServices } from '../app/context';
import { DialogLoading } from '../dialogs/DialogShell';
import { Icon, type IconName } from '../icons/Icon';
import {
  ChecksRefreshMs, ciCounts, ciGlyph, followUpHeader, partTone, threadPlace, threadsFollowUp, unknownReason, warningLines, warns,
  type ChecksModel, type ChecksPart, type ChecksWarning,
} from './checksModel';
import { prStateLocaleKey, safeExternalHttpUrl } from './gitWorkbenchModel';

export interface ChecksTabProps {
  workspaceId: string | null;
  paneId: string | null;
  /** The tab is shown and the panel open: only then is anything read. */
  active: boolean;
  /** Changes when the panel's refresh button is pressed. */
  refreshNonce: number;
  busy: boolean;
  /** Ask the agent to fix the failed checks of the pull request at `url`. */
  onFixChecks: (url: string) => void;
  /** The follow-up into the pane's composer (not sent). */
  onToComposer: (text: string) => void;
}

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

function Part({ id, icon, title, part, warn, line, actions, children }: {
  id: string; icon: IconName; title: string; part: ChecksPart<unknown>; warn: boolean; line: ReactNode; actions?: ReactNode; children?: ReactNode;
}) {
  const glyph = partTone(part.ok, warn);
  return (
    <section className="git-ck-part" data-testid={'git-checks-' + id} data-state={part.ok ? (warn ? 'warn' : 'ok') : 'unknown'}>
      <header className="git-ck-head">
        <span className={'git-ck-glyph tone-text-' + glyph.tone} aria-hidden="true"><Icon name={glyph.icon} size={15} weight={2} /></span>
        <Icon name={icon} size={13} weight={2} />
        <strong className="git-ck-title">{title}</strong>
        <span className="git-ck-line" title={!part.ok ? part.detail : undefined}>{line}</span>
        <span className="spacer" />
        {actions}
      </header>
      {part.ok && children}
    </section>
  );
}

function unknownLine(part: ChecksPart<unknown>, t: Translate): ReactNode {
  if (part.ok) return null;
  return <><span className="tone-text-wait">{t('git.checks.unknown')}</span> · {unknownReason(part, t)}</>;
}

/** The tab's contents for one model (drawn on its own for the tests). */
export function ChecksView({ model, t, busy, selected, setSelected, open, onFixChecks, onToComposer }: {
  model: ChecksModel; t: Translate; busy: boolean; selected: ReadonlySet<string>; setSelected: (next: Set<string>) => void;
  open: (url: string) => void; onFixChecks: (url: string) => void; onToComposer: (text: string) => void;
}) {
  const { git, pr, checks, threads, checklist, warnings } = model;
  const prValue = pr.ok ? pr.value : null;
  const prUrl = safeExternalHttpUrl(prValue?.url);
  const threadList = threads.ok ? threads.value : [];
  const chosen = threadList.filter((thread) => selected.has(thread.id));
  const allChosen = threadList.length > 0 && chosen.length === threadList.length;
  const counts = checks.ok ? ciCounts(checks.value) : null;

  const gitLine = git.ok ? (
    <>
      {git.value.uncommitted > 0 ? t('git.checks.warn.uncommitted', { count: git.value.uncommitted }) : t('git.checks.git.clean')}
      {git.value.conflicted > 0 && <> · <span className="tone-text-err">{t('git.checks.git.conflicted', { count: git.value.conflicted })}</span></>}
      {git.value.base && <> · <span className="mono">{t('git.checks.git.base', { base: git.value.base, ahead: git.value.ahead, behind: git.value.behind })}</span></>}
    </>
  ) : unknownLine(git, t);

  const stateKey = prValue ? prStateLocaleKey(prValue.state) : null;
  const flags = prValue ? [
    prValue.isDraft ? t('panePr.draft') : '',
    prValue.mergeable === 'conflicting' ? t('git.checks.pr.conflicts') : '',
    prValue.reviewDecision === 'changesRequested' ? t('git.checks.pr.changesRequested')
      : prValue.reviewDecision === 'approved' ? t('git.checks.pr.approved')
        : prValue.reviewDecision === 'reviewRequired' ? t('git.checks.pr.reviewRequired') : '',
  ].filter((flag) => flag !== '') : [];
  const prLine = prValue ? (
    <>
      {t('git.pr.line', { number: prValue.number, state: stateKey ? t(stateKey) : prValue.state })}
      {flags.length > 0 && ' · ' + flags.join(' · ')}
      {prValue.title && <span className="git-ck-pr-title" title={prValue.title}>{prValue.title}</span>}
    </>
  ) : unknownLine(pr, t);

  return (
    <div className="git-ck-list">
      <Part id="git" icon="file-code" title={t('git.checks.git.title')} part={git} warn={warns(warnings, 'uncommitted')} line={gitLine} />

      <Part id="pr" icon="git-merge" title={t('git.checks.pr.title')} part={pr} warn={warns(warnings, 'draft', 'conflicts', 'changesRequested')} line={prLine}
        actions={prUrl && (
          <button type="button" className="btn git-wb-compact-btn" data-testid="git-checks-pr-open" onClick={() => open(prUrl)}>
            <Icon name="external-link" size={12} weight={2} /><span>{t('git.pr.open')}</span>
          </button>
        )} />

      <Part id="ci" icon="workflow" title={t('git.checks.ci.title')} part={checks} warn={warns(warnings, 'checksFailed', 'checksPending')}
        line={counts ? (checks.ok && checks.value.length === 0 ? t('git.checks.ci.none') : t('git.checks.ci.summary', { passed: counts.passed, failed: counts.failed, pending: counts.pending })) : unknownLine(checks, t)}
        actions={counts && counts.failed > 0 && prValue && (
          <button type="button" className="btn git-wb-compact-btn git-wb-fix-btn" data-testid="git-checks-fix" disabled={busy} onClick={() => onFixChecks(prValue.url)}>
            <Icon name="wrench" size={12} weight={2} /><span>{t('git.pr.fixChecks')}</span>
          </button>
        )}>
        {checks.ok && checks.value.length > 0 && (
          <ul className="git-ck-rows">
            {checks.value.map((item, index) => {
              const glyph = ciGlyph(item.state);
              const url = safeExternalHttpUrl(item.url);
              const word = item.state === 'passed' ? t('git.pr.checks.passed') : item.state === 'failed' ? t('git.pr.checks.failed')
                : item.state === 'pending' ? t('git.pr.checks.pending') : t('git.checks.ci.skipped');
              return (
                <li key={index} className="git-ck-row" data-state={item.state}>
                  <span className={'tone-text-' + glyph.tone} aria-hidden="true"><Icon name={glyph.icon} size={12} weight={2} /></span>
                  <span className="git-ck-row-main" title={item.name}>{item.name}</span>
                  <span className={'git-ck-row-side tone-text-' + glyph.tone}>{word}</span>
                  {url && (
                    <button type="button" className="icon-button git-wb-row-icon" aria-label={t('git.pr.open')} title={t('git.pr.open')} onClick={() => open(url)}>
                      <Icon name="external-link" size={11} weight={2} />
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </Part>

      <Part id="threads" icon="message-square-text" title={t('git.checks.threads.title')} part={threads} warn={warns(warnings, 'threads')}
        line={threads.ok ? (threadList.length === 0 ? t('git.checks.threads.none') : t('git.checks.warn.threads', { count: threadList.length })) : unknownLine(threads, t)}
        actions={threadList.length > 0 && (
          <>
            <label className="git-wb-select-all">
              <input type="checkbox" data-testid="git-checks-threads-all" checked={allChosen}
                onChange={() => setSelected(allChosen ? new Set() : new Set(threadList.map((thread) => thread.id)))} />
              <span>{t('git.workbench.selectAll')}</span>
            </label>
            <button type="button" className="btn primary git-wb-compact-btn" data-testid="git-checks-to-composer" title={t('git.checks.threads.toComposerHelp')}
              disabled={chosen.length === 0 || !prValue}
              onClick={() => { if (prValue) onToComposer(threadsFollowUp(followUpHeader(prValue, t), chosen)); }}>
              <Icon name="corner-down-right" size={12} weight={2} /><span>{t('git.checks.threads.toComposer')}</span>
            </button>
          </>
        )}>
        {threadList.length > 0 && (
          <ul className="git-ck-rows">
            {threadList.map((thread) => {
              const url = safeExternalHttpUrl(thread.url);
              const place = threadPlace(thread);
              const first = thread.body.split('\n').find((line) => line.trim() !== '') ?? '';
              return (
                <li key={thread.id} className="git-ck-row git-ck-thread" data-testid={'git-checks-thread-' + thread.id}>
                  <input type="checkbox" aria-label={place || thread.author} checked={selected.has(thread.id)}
                    onChange={(event) => {
                      const next = new Set(selected);
                      if (event.target.checked) next.add(thread.id); else next.delete(thread.id);
                      setSelected(next);
                    }} />
                  <div className="git-ck-thread-words">
                    <div className="git-ck-thread-head">
                      {place && <span className="mono git-ck-place" title={place}>{place}</span>}
                      {thread.author && <span className="git-ck-author">{thread.author}</span>}
                      {thread.outdated && <span className="git-ck-badge">{t('git.checks.threads.outdated')}</span>}
                      {thread.replies > 0 && <span className="git-ck-row-side">{t('git.checks.threads.replies', { count: thread.replies })}</span>}
                    </div>
                    <span className="git-ck-thread-body selectable" title={thread.body}>{first}</span>
                  </div>
                  {url && (
                    <button type="button" className="icon-button git-wb-row-icon" aria-label={t('git.pr.open')} title={t('git.pr.open')} onClick={() => open(url)}>
                      <Icon name="external-link" size={11} weight={2} />
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </Part>

      <Part id="checklist" icon="list-todo" title={t('git.checks.checklist.title')} part={checklist} warn={warns(warnings, 'checklist')}
        line={checklist.ok ? (checklist.value.total === 0 ? t('git.checks.checklist.noList')
          : checklist.value.remaining.length === 0 ? t('git.checks.checklist.none')
            : t('git.checks.warn.checklist', { count: checklist.value.remaining.length })) : unknownLine(checklist, t)}>
        {checklist.ok && checklist.value.remaining.length > 0 && (
          <ul className="git-ck-rows">
            {checklist.value.remaining.map((item, index) => (
              <li key={index} className="git-ck-row">
                <Icon name="circle" size={11} weight={2} />
                <span className="git-ck-row-main" title={item}>{item}</span>
              </li>
            ))}
          </ul>
        )}
      </Part>
    </div>
  );
}

/** The summary over the parts: ✓, or the warnings on one line. */
export function ChecksSummary({ warnings, t }: { warnings: readonly ChecksWarning[]; t: Translate }) {
  if (warnings.length === 0) {
    return <span className="git-ck-summary tone-text-done" data-testid="git-checks-summary"><Icon name="circle-check" size={13} weight={2} />{t('review.checks.ok')}</span>;
  }
  return (
    <span className="git-ck-summary tone-text-err" data-testid="git-checks-summary">
      <Icon name="triangle-alert" size={13} weight={2} />{warningLines(warnings, t).join(' · ')}
    </span>
  );
}

export function ChecksTab({ workspaceId, paneId, active, refreshNonce, busy, onFixChecks, onToComposer }: ChecksTabProps) {
  const { bridge, t } = useServices();
  const [model, setModel] = useState<ChecksModel | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const reading = useRef(false);

  useEffect(() => {
    if (!active || !workspaceId) return undefined;
    let live = true;
    const read = () => {
      if (reading.current || (typeof document !== 'undefined' && document.visibilityState === 'hidden')) return;
      reading.current = true;
      setLoading(true);
      void bridge.call<ChecksModel>('gitWorkbenchChecks', { workspaceId, paneId }).then(
        (next) => {
          if (!live || !next) return;
          setModel(next);
          setError(null);
          const ids = new Set(next.threads.ok ? next.threads.value.map((thread) => thread.id) : []);
          setSelected((prev) => new Set([...prev].filter((id) => ids.has(id))));
        },
        (reason: unknown) => { if (live) setError(reason instanceof Error ? reason.message : String(reason)); },
      ).finally(() => {
        reading.current = false;
        if (live) setLoading(false);
      });
    };
    read();
    const timer = setInterval(read, ChecksRefreshMs);
    return () => { live = false; clearInterval(timer); };
  }, [active, bridge, workspaceId, paneId, refreshNonce]);

  const open = (url: string) => { void bridge.call('openUrl', { url }).catch(() => {}); };

  return (
    <div className="git-ck-body" data-testid="git-checks-tab">
      <div className="git-ck-top">
        {model && <ChecksSummary warnings={model.warnings} t={t} />}
        <span className="spacer" />
        {loading && <span className="dialog-spinner" aria-hidden="true" />}
        <span className="git-ck-note">{t('git.checks.autoRefresh')}</span>
      </div>
      {error && <div className="tone-text-err selectable git-ck-error">{error}</div>}
      {model ? (
        <ChecksView model={model} t={t} busy={busy} selected={selected} setSelected={setSelected} open={open} onFixChecks={onFixChecks} onToComposer={onToComposer} />
      ) : !error && <DialogLoading rows={5} />}
    </div>
  );
}

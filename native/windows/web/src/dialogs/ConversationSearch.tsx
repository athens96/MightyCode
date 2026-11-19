// 대화 검색 (M/ConversationSearchSheet.swift; the host's conversation_search.rs): the workspace's past sessions whose requests and
// answers hold every word typed. Each keystroke restarts the search after a short pause (the host stops the one before); ↑/↓ choose
// a hit, Enter continues it in a new pane (the sheet's answer `{resume: id}`), and a hit can be saved as Markdown.
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { useServices } from '../app/context';
import { ProviderMark } from '../conversation/ProviderMark';
import { Icon } from '../icons/Icon';
import { Pending } from '../pending/Pending';
import { MiddlePath } from '../shell/parts';
import type { ConversationHit, ConversationResults, DialogRequest } from '../shell/types';
import { DialogLoading } from './DialogShell';
import { Sheet } from './Dialogs';
import { highlight, moveSelection, searchTerms } from './conversationSearchRules';

/** The pause after a keystroke before the search starts (Swift's 250 ms). */
export const SEARCH_PAUSE_MS = 250;

function Snippet({ text, query }: { text: string; query: string }) {
  return <span className="conversation-hit-snippet selectable">{highlight(text, query).map((part, index) => part.match ? <mark key={index}>{part.text}</mark> : part.text)}</span>;
}

export function Hit({ hit, query, selected, onSelect, onResume, onExport }: {
  hit: ConversationHit; query: string; selected: boolean; onSelect: () => void; onResume: () => void; onExport: () => void;
}) {
  return (
    <div className={'conversation-hit' + (selected ? ' selected' : '')} role="option" aria-selected={selected} data-testid="conversation-hit" data-hit-id={hit.id}
      onClick={onSelect} onDoubleClick={onResume}>
      <ProviderMark provider={hit.provider} size={14} />
      <div className="conversation-hit-text">
        <div className="conversation-hit-head">
          <span className="conversation-hit-title" title={hit.title}>{hit.title}</span>
          <span className="quiet small conversation-hit-date">{hit.agent} · {hit.date}</span>
        </div>
        <Snippet text={hit.snippet} query={query} />
        <div className="conversation-hit-foot">
          <span className="quiet small">{hit.matches}</span>
          <span className="conversation-hit-actions">
            <ExportButton onExport={onExport} />
            <ResumeButton id={hit.id} onResume={onResume} />
          </span>
        </div>
      </div>
    </div>
  );
}

function ExportButton({ onExport }: { onExport: () => void }) {
  const { t } = useServices();
  return <button type="button" className="btn text small" tabIndex={-1} onClick={(event) => { event.stopPropagation(); onExport(); }}>{t('conversationSearch.export')}</button>;
}

function ResumeButton({ id, onResume }: { id: string; onResume: () => void }) {
  const { t } = useServices();
  return <button type="button" className="btn text small strong" tabIndex={-1} data-testid={'conversation-resume-' + id} onClick={(event) => { event.stopPropagation(); onResume(); }}>{t('conversationSearch.resume')}</button>;
}

export function ConversationSearch({ id, dialog, answer }: { id: number; dialog: Extract<DialogRequest, { type: 'conversationSearch' }>; answer: (value: unknown) => void }) {
  const { bridge, t } = useServices();
  const [query, setQuery] = useState('');
  // Null until the folder's sessions are read.
  const [results, setResults] = useState<ConversationResults | null>(null);
  const [searched, setSearched] = useState('');
  const [searching, setSearching] = useState(false);
  const [selected, setSelected] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    const words = searchTerms(query).length > 0;
    setSearching(words);
    const timer = setTimeout(() => {
      void bridge.call<ConversationResults | null>('conversationSearch', { query: words ? query : '' }).then((value) => {
        // Null: the sheet closed, or a newer query took over.
        if (!live || !value) return;
        setResults(value); setSearched(words ? query : ''); setSelected(0); setSearching(false);
      }, (reason: unknown) => { if (live) { setSearching(false); setError(reason instanceof Error ? reason.message : String(reason)); } });
    }, words ? SEARCH_PAUSE_MS : 0);
    return () => { live = false; clearTimeout(timer); };
  }, [bridge, query]);

  useEffect(() => {
    list.current?.querySelector('.conversation-hit.selected')?.scrollIntoView({ block: 'nearest' });
  }, [selected, results]);

  const hits = searchTerms(query).length > 0 ? results?.hits ?? [] : [];
  const resume = (hit: ConversationHit | undefined) => { if (hit) answer({ resume: hit.id }); };
  const save = (hit: ConversationHit) => {
    setError(null);
    void bridge.call('conversationExport', { id: hit.id }).catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)));
  };
  const key = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Enter') { event.preventDefault(); resume(hits[selected]); return; }
    const next = moveSelection(selected, event.key, hits.length);
    if (next !== null) { event.preventDefault(); setSelected(next); }
  };

  let body;
  if (results === null) body = <DialogLoading rows={4} />;
  else if (searchTerms(query).length === 0) body = <div className="resume-empty"><span className="quiet">{t('conversationSearch.hint', { count: results.count })}</span></div>;
  else if (hits.length === 0 && !searching) body = <div className="resume-empty"><strong>{t('conversationSearch.none')}</strong></div>;
  else body = hits.map((hit, index) => (
    <Hit key={hit.id} hit={hit} query={searched} selected={index === selected} onSelect={() => setSelected(index)} onResume={() => resume(hit)} onExport={() => save(hit)} />
  ));

  return (
    <Sheet id={id} width={560} label={dialog.title} icon={<Icon name="search" size={18} />} subtitle={<MiddlePath path={dialog.path} className="mono" />}
      onCancel={() => answer(null)} status={error ? <span className="tone-text-err small">{error}</span> : <span className="quiet small">{t('conversationSearch.note')}</span>}
      buttons={<button type="button" className="btn" onClick={() => answer(null)}>{t('dialog.close')}</button>}>
      <div className="resume-list conversation-search" style={{ height: 440 }} data-testid="conversation-search">
        <label className="resume-search">
          <Icon name="search" size={13} weight={2} />
          <input className="dialog-field sheet-field" type="search" data-autofocus placeholder={t('conversationSearch.placeholder')} aria-label={t('conversationSearch.placeholder')}
            value={query} data-testid="conversation-search-field" onChange={(event) => setQuery(event.target.value)} onKeyDown={key} />
          {searching && <span className="conversation-search-busy"><Pending busy showsLabel={false} /></span>}
        </label>
        <div className="resume-rows" role="listbox" aria-label={dialog.title} ref={list} data-testid="conversation-hits">{body}</div>
        {hits.length > 0 && <span className="quiet small">{t('conversationSearch.keys')}</span>}
      </div>
    </Sheet>
  );
}

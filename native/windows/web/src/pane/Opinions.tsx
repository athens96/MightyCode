import { memo } from 'react';
import { useServices, useStoreSelector } from '../app/context';
import { Icon } from '../icons/Icon';
import { Markdown } from '../conversation/MarkdownView';
import { ProviderMark } from '../conversation/ProviderMark';
import { providerLabel } from '../conversation/format';
import type { ForkedFrom, OpinionCard } from '../store/models';
import { useChoosePane } from '../shell/Sidebar';
import { opinionFigures, opinionGroups, opinionInsertText } from './opinionCards';
import './opinionCards.css';

/**
 * MC-28: a forked pane's strip over its conversation: where it came from (the source's title now, or as saved when it is closed) and
 * how the conversation came along, with 원본 창 보기 while the source is open.
 */
export const ForkStrip = memo(function ForkStrip({ link }: { link: ForkedFrom }) {
  const { t } = useServices();
  const choose = useChoosePane();
  const source = useStoreSelector((state) => state.shell?.panes.find((p) => p.id === link.paneId)?.title);
  const title = source ?? link.title ?? t('fork.strip.closed');
  const how = link.forkPending ? t('fork.strip.cli') : link.handoff ? t('fork.strip.handoff') : null;
  return (
    <div className="fork-strip" data-testid="fork-strip">
      <Icon name="git-fork" size={13} weight={2.2} />
      <span className="fork-strip-text">{t('fork.label', { title })}</span>
      {how && <span className="fork-strip-how">· {how}</span>}
      <span className="spacer" />
      {source !== undefined && (
        <button type="button" className="btn small" data-testid="fork-strip-open" onClick={() => choose(link.paneId)}>{t('fork.strip.open')}</button>
      )}
    </div>
  );
});

function Card({ paneId, card }: { paneId: string; card: OpinionCard }) {
  const { bridge, store, t } = useServices();
  const agent = providerLabel(card.provider);
  const figures = opinionFigures(card);
  const insert = () => store.dispatch({ kind: 'fill', paneId, text: opinionInsertText(card, t), append: true });
  return (
    <article className={'opinion-card state-' + card.state} data-testid="opinion-card" data-state={card.state}>
      <header className="opinion-head">
        <ProviderMark provider={card.provider} size={13} />
        <span className="opinion-title">{t('advisor.card.title', { agent })}</span>
        {figures && <span className="opinion-figures">{figures}</span>}
        <span className="spacer" />
        {card.state !== 'running' && (
          <button type="button" className="icon-button" title={t('advisor.card.dismiss')} aria-label={t('advisor.card.dismiss')} data-testid="opinion-dismiss"
            onClick={() => void bridge.call('opinionDismiss', { paneId, cardId: card.id }).catch(() => {})}><Icon name="x" size={13} weight={2.2} /></button>
        )}
      </header>
      {card.question && <div className="opinion-question">{t('advisor.card.question', { question: card.question })}</div>}
      <div className="opinion-body selectable">
        {card.state === 'running'
          ? <span className="opinion-waiting"><Icon name="loader-circle" size={13} weight={2} /> {t('advisor.card.running')}</span>
          : card.state === 'failed'
            ? <span className="opinion-failed">{t('advisor.card.failed')}{card.body ? ': ' + card.body : ''}</span>
            : <Markdown text={card.body} paneId={paneId} />}
      </div>
      {card.state === 'done' && (
        <footer className="opinion-foot">
          <button type="button" className="btn small" data-testid="opinion-insert" onClick={insert}>{t('advisor.card.insert')}</button>
        </footer>
      )}
    </article>
  );
}

/** MC-28: the pane's opinion cards under its conversation; the two of one /committee side by side. */
export const Opinions = memo(function Opinions({ paneId, cards }: { paneId: string; cards: readonly OpinionCard[] }) {
  const { t } = useServices();
  if (cards.length === 0) return null;
  return (
    <section className="opinions" aria-label={t('advisor.cards.label')} data-testid="opinions">
      {opinionGroups(cards).map((group) => (
        <div key={group[0]?.id ?? ''} className={'opinion-row' + (group.length > 1 ? ' committee' : '')}>
          {group.map((card) => <Card key={card.id} paneId={paneId} card={card} />)}
        </div>
      ))}
    </section>
  );
});

// MC-28: the opinion cards' rules: how they group (a /committee's two side by side), the figures their header shows and the words
// 입력창에 넣기 puts into the composer.
import { providerLabel } from '../conversation/format';
import type { Translate } from '../i18n/locale';
import type { OpinionCard } from '../store/models';

/** The cards in their order, each /committee's cards (one `group`) together where the first of them stands. */
export function opinionGroups(cards: readonly OpinionCard[]): OpinionCard[][] {
  const rows: OpinionCard[][] = [];
  const byGroup = new Map<string, OpinionCard[]>();
  for (const card of cards) {
    if (card.group) {
      const row = byGroup.get(card.group);
      if (row) { row.push(card); continue; }
      const made = [card];
      byGroup.set(card.group, made);
      rows.push(made);
    } else rows.push([card]);
  }
  return rows;
}

/** "gpt-6-astra · $0.02 · 1.2k tokens": what the CLI told of the answer; empty when it told nothing. */
export function opinionFigures(card: OpinionCard): string {
  const parts: string[] = [];
  if (card.model) parts.push(card.model);
  if (card.costUsd !== undefined && card.costUsd > 0) parts.push('$' + (card.costUsd < 0.01 ? card.costUsd.toFixed(4) : card.costUsd.toFixed(2)));
  if (card.tokens !== undefined && card.tokens > 0) parts.push((card.tokens >= 1000 ? (card.tokens / 1000).toFixed(1) + 'k' : String(card.tokens)) + ' tokens');
  return parts.join(' · ');
}

/** What 입력창에 넣기 adds to the composer: the agent's opinion, introduced so the pane's agent knows whose it is. */
export function opinionInsertText(card: OpinionCard, t: Translate): string {
  return t('advisor.insert.text', { agent: providerLabel(card.provider), body: card.body.trim() });
}

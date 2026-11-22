import { describe, expect, it } from 'vitest';
import { opinionFigures, opinionGroups, opinionInsertText } from './opinionCards';
import type { OpinionCard } from '../store/models';

const card = (id: string, extra: Partial<OpinionCard> = {}): OpinionCard => ({ id, provider: 'codex', body: 'Use a lock.', state: 'done', ...extra });

describe('opinion cards', () => {
  it('puts the two cards of a committee side by side where the first stands', () => {
    const groups = opinionGroups([card('a'), card('b', { group: 'g' }), card('c'), card('d', { group: 'g' })]);
    expect(groups.map((g) => g.map((c) => c.id))).toEqual([['a'], ['b', 'd'], ['c']]);
  });

  it('shows the model, cost and tokens the CLI told', () => {
    expect(opinionFigures(card('a', { model: 'gpt-6-astra', costUsd: 0.0234, tokens: 1234 }))).toBe('gpt-6-astra · $0.02 · 1.2k tokens');
    expect(opinionFigures(card('a', { costUsd: 0.0012 }))).toBe('$0.0012');
    expect(opinionFigures(card('a'))).toBe('');
  });

  it('puts the opinion into the composer with whose it is', () => {
    const t = (key: string, subs?: Readonly<Record<string, string | number>>) => key + ':' + JSON.stringify(subs);
    expect(opinionInsertText(card('a', { body: '  Use a lock.\n' }), t)).toBe('advisor.insert.text:{"agent":"Codex","body":"Use a lock."}');
  });
});

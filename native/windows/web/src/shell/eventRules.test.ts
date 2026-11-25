import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { EventCard } from './ReportBox';
import reportBoxCss from './reportBox.css?raw';
import { eventDays, eventTone, outcomeWords, shownFields, triggerRows, type EventRecord, type TriggerOutcome } from './eventRules';

const t = (key: string, subs?: Readonly<Record<string, string | number>>) => key + (subs ? JSON.stringify(subs) : '');
const at = (date: Date, hours: number) => new Date(date.getFullYear(), date.getMonth(), date.getDate(), hours).toISOString();
const now = new Date(2026, 10, 25, 18, 0);

const outcome = (key: string, o: TriggerOutcome['outcome'], more: Partial<TriggerOutcome> = {}): TriggerOutcome => ({
  key, name: key, workspace: 'Shop', outcome: o, at: at(now, 10), ...more,
});
const record = (id: string, time: string, triggers: TriggerOutcome[]): EventRecord => ({
  id,
  event: {
    id: 'pr:7', source: 'github:o/r', type: 'github.pr.opened', subject: 'PR#7', time, actor: 'kim', title: 'PR #7: Fix', detail: '',
    fields: { author: 'kim', labels: 'bug', 'event.type': 'github.pr.opened', 'pr.title': 'Fix' },
  },
  triggers,
});

describe('the 알림 함 events (MC-97)', () => {
  it('says how an event went: fired when any trigger fired, else skipped, quiet when none listened', () => {
    expect(eventTone(record('a', at(now, 9), [outcome('x', 'unmatched'), outcome('y', 'fired')]))).toBe('fired');
    expect(eventTone(record('b', at(now, 9), [outcome('x', 'asleep')]))).toBe('skipped');
    expect(eventTone(record('c', at(now, 9), []))).toBe('quiet');
  });

  it('says why each trigger skipped, with the condition and the value seen', () => {
    const miss = { field: 'labels', op: 'has', expected: 'bot:dev', seen: 'bug' };
    expect(outcomeWords(outcome('x', 'fired'), t)).toBe('✓ automation.events.fired');
    expect(outcomeWords(outcome('x', 'unmatched', { miss }), t)).toBe('✗ automation.events.unmatched{"condition":"labels has bot:dev","seen":"bug"}');
    expect(outcomeWords(outcome('x', 'unmatched', { miss: { ...miss, seen: null } }), t)).toContain('"seen":"automation.events.none"');
    expect(outcomeWords(outcome('x', 'asleep', { reason: '@콩이는 자는 중입니다.' }), t)).toBe('✗ @콩이는 자는 중입니다.');
    expect(outcomeWords(outcome('x', 'budget'), t)).toBe('✗ automation.events.budget');
    expect(outcomeWords(outcome('x', 'held'), t)).toBe('✗ automation.events.held');
  });

  it('lists each trigger once with its arrival and its 시험 실행 in order, and the events by day, newest first', () => {
    const r = record('a', at(now, 9), [outcome('x', 'unmatched'), outcome('y', 'fired'), outcome('x', 'fired', { replay: true })]);
    const rows = triggerRows(r);
    expect(rows.map((row) => [row.key, row.outcomes.map((o) => o.outcome)])).toEqual([['x', ['unmatched', 'fired']], ['y', ['fired']]]);
    const days = eventDays([record('old', at(new Date(2026, 10, 20), 9), []), record('a', at(now, 9), []), record('b', at(now, 11), [])], now);
    expect(days.map((d) => [d.key, d.events.map((e) => e.id)])).toEqual([['today', ['b', 'a']], ['2026-11-20', ['old']]]);
    expect(shownFields(r.event)).toEqual([['author', 'kim'], ['labels', 'bug'], ['pr.title', 'Fix']]);
  });

  it('draws a card with a status dot and chip, never a colored left edge, and 시험 실행 per trigger when unfolded', () => {
    const services = { t: translator([{ 'automation.events.replay': '시험 실행', 'automation.events.tone.fired': '실행' }]) } as unknown as AppServices;
    const render = (expanded: boolean) => renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(EventCard, {
      record: record('a', at(now, 9), [outcome('review', 'fired', { run: 'r1' }), outcome('triage', 'unmatched')]),
      expanded, onToggle: () => {}, onReplay: () => {}, onOpenUrl: () => {},
    })));
    const folded = render(false);
    expect(folded).toContain('tone-dot dot-done');
    expect(folded).toContain('github.pr.opened');
    expect(folded).toContain('PR#7');
    expect(folded).not.toContain('event-replay');
    const open = render(true);
    expect(open.match(/data-testid="event-replay"/g)).toHaveLength(2);
    expect(open).toContain('시험 실행');
    const eventRules = [...reportBoxCss.matchAll(/\.event-[a-z-]+\b[^{]*\{[^}]*\}/g)].map((m) => m[0]).join('\n');
    expect(eventRules).not.toMatch(/\bborder-left\b|\bborder-inline-start\b/);
    expect(eventRules).not.toMatch(/box-shadow\s*:[^;]*\binset\s+[1-9]/);
  });
});

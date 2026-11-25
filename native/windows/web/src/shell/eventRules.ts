// MC-97: the 알림 함's 이벤트 tab: the events that reached the automation layer (the host's event_inbox.rs), each with its envelope
// (type, subject, source, time, actor) and what every trigger listening to it did: ✓ fired, or ✗ and why (the `where` condition that
// failed and the value seen, the bot asleep, over budget, held back, failed to start; MC-98: its source could not be read). Pure
// rules; ReportBox.tsx draws them.
import { dayKey } from './reportRules';

export type EventOutcome = 'fired' | 'unmatched' | 'asleep' | 'budget' | 'held' | 'failed' | 'unreachable';

/** The `where` condition an event did not pass, and the value it had for that field (none when it had none). */
export interface ConditionMiss {
  field: string;
  op: string;
  expected: string;
  seen?: string | null;
}

export interface TriggerOutcome {
  /** The trigger's job key (시험 실행 names it). */
  key: string;
  name: string;
  workspace: string;
  outcome: EventOutcome;
  miss?: ConditionMiss | null;
  reason?: string | null;
  run?: string | null;
  /** RFC 3339. */
  at: string;
  /** A person's 시험 실행. */
  replay?: boolean;
}

/** The normalized event: where it came from, what happened, what it is about, when and by whom. */
export interface InboxEvent {
  id: string;
  source: string;
  type: string;
  subject: string;
  /** RFC 3339. */
  time: string;
  actor?: string | null;
  title: string;
  url?: string | null;
  detail: string;
  fields: Record<string, string>;
}

export interface EventRecord {
  id: string;
  event: InboxEvent;
  triggers: TriggerOutcome[];
}

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

/** How an event went, for its dot and chip: some trigger fired, every one skipped it, or none listened. */
export type EventTone = 'fired' | 'skipped' | 'quiet';

export function eventTone(record: EventRecord): EventTone {
  if (record.triggers.some((t) => t.outcome === 'fired')) return 'fired';
  return record.triggers.length > 0 ? 'skipped' : 'quiet';
}

/** The dot's tone class (mighty.css `.tone-dot`): fired green, skipped amber, none listening neutral. */
export const eventDots: Record<EventTone, string> = { fired: 'dot-done', skipped: 'dot-wait', quiet: '' };
export const eventToneKeys: Record<EventTone, string> = {
  fired: 'automation.events.tone.fired',
  skipped: 'automation.events.tone.skipped',
  quiet: 'automation.events.tone.quiet',
};

/** A condition in words: `labels has bot:dev`. */
export function conditionWords(miss: ConditionMiss): string {
  return `${miss.field} ${miss.op} ${miss.expected}`;
}

/** What one trigger did, in words: ✓ fired, or ✗ and why. */
export function outcomeWords(o: TriggerOutcome, t: Translate): string {
  switch (o.outcome) {
    case 'fired': return `✓ ${t('automation.events.fired')}`;
    case 'unmatched': return o.miss
      ? `✗ ${t('automation.events.unmatched', { condition: conditionWords(o.miss), seen: o.miss.seen ?? t('automation.events.none') })}`
      : `✗ ${t('automation.events.unmatchedPlain')}`;
    case 'asleep': return `✗ ${o.reason || t('automation.events.asleep')}`;
    case 'budget': return `✗ ${o.reason || t('automation.events.budget')}`;
    case 'held': return `✗ ${t('automation.events.held')}`;
    case 'failed': return `✗ ${o.reason || t('automation.events.failed')}`;
    case 'unreachable': return `✗ ${o.reason || t('automation.events.unreachable')}`;
  }
}

/** MC-98: whether 시험 실행 has anything to run for a trigger: not when every outcome says its source could not be read. */
export function replayable(row: TriggerRow): boolean {
  return row.outcomes.some((o) => o.outcome !== 'unreachable');
}

/** One trigger an event reached, with what it did each time (its arrival first, then any 시험 실행), in order. */
export interface TriggerRow {
  key: string;
  name: string;
  workspace: string;
  outcomes: TriggerOutcome[];
}

export function triggerRows(record: EventRecord): TriggerRow[] {
  const rows = new Map<string, TriggerRow>();
  for (const o of record.triggers) {
    const row = rows.get(o.key) ?? { key: o.key, name: o.name, workspace: o.workspace, outcomes: [] };
    row.outcomes.push(o);
    rows.set(o.key, row);
  }
  return [...rows.values()];
}

/** The events by day (today, yesterday, then the date), newest day first and newest first within it. */
export function eventDays(records: readonly EventRecord[], now: Date): { key: string; events: EventRecord[] }[] {
  const days = new Map<string, EventRecord[]>();
  for (const r of [...records].sort((a, b) => b.event.time.localeCompare(a.event.time))) {
    const key = dayKey(r.event.time, now);
    days.set(key, [...(days.get(key) ?? []), r]);
  }
  return [...days.entries()].map(([key, events]) => ({ key, events }));
}

/** The fields a condition may read, the envelope's `event.*` copies left out (`type`, `subject`, … read them), in name order. */
export function shownFields(event: InboxEvent): [string, string][] {
  return Object.entries(event.fields ?? {}).filter(([key]) => !key.startsWith('event.')).sort(([a], [b]) => a.localeCompare(b));
}

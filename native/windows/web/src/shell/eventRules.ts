// MC-97: the 알림 함's 이벤트 tab: the events that reached the automation layer (the host's event_inbox.rs), each with its envelope
// (type, subject, source, time, actor) and what every trigger listening to it did: ✓ fired, or ✗ and why (the `where` condition that
// failed and the value seen, the bot asleep, over budget, held back, failed to start; MC-98: its source could not be read; MC-99: its
// policy skipped it, it waits for a run to end, or for the person's approval). Pure rules; ReportBox.tsx draws them, and the
// approvals waiting (실행 / 건너뛰기) above the events.
import { dayKey } from './reportRules';

export type EventOutcome = 'fired' | 'unmatched' | 'asleep' | 'budget' | 'held' | 'failed' | 'unreachable' | 'skipped' | 'awaiting';

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

/** MC-99: an action waiting for the person's 실행 / 건너뛰기 (the host's automation/policy.rs). */
export interface Approval {
  id: string;
  /** The trigger's job key. */
  key: string;
  name: string;
  workspace: string;
  /** The event inbox record it is noted under. */
  inbox: string;
  event: InboxEvent;
  /** What will run, in words. */
  what: string;
  /** RFC 3339. */
  at: string;
}

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

/** How an event went, for its dot and chip: some trigger fired, one waits for approval (MC-99), every one skipped it, or none listened. */
export type EventTone = 'fired' | 'awaiting' | 'skipped' | 'quiet';

/** A trigger's last word on the event (its 시험 실행 and a decided approval come after its arrival). */
function lastOutcomes(record: EventRecord): EventOutcome[] {
  return triggerRows(record).map((row) => row.outcomes[row.outcomes.length - 1]!.outcome);
}

export function eventTone(record: EventRecord): EventTone {
  if (record.triggers.some((t) => t.outcome === 'fired')) return 'fired';
  if (lastOutcomes(record).includes('awaiting')) return 'awaiting';
  return record.triggers.length > 0 ? 'skipped' : 'quiet';
}

/** The dot's tone class (mighty.css `.tone-dot`): fired green, waiting and skipped amber, none listening neutral. */
export const eventDots: Record<EventTone, string> = { fired: 'dot-done', awaiting: 'dot-wait', skipped: 'dot-wait', quiet: '' };
export const eventToneKeys: Record<EventTone, string> = {
  fired: 'automation.events.tone.fired',
  awaiting: 'automation.events.tone.awaiting',
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
    // MC-99: an emit says which event it sent.
    case 'fired': return `✓ ${t('automation.events.fired')}${o.reason ? ` · ${o.reason}` : ''}`;
    case 'unmatched': return o.miss
      ? `✗ ${t('automation.events.unmatched', { condition: conditionWords(o.miss), seen: o.miss.seen ?? t('automation.events.none') })}`
      : `✗ ${t('automation.events.unmatchedPlain')}`;
    case 'asleep': return `✗ ${o.reason || t('automation.events.asleep')}`;
    case 'budget': return `✗ ${o.reason || t('automation.events.budget')}`;
    // MC-99: or it waits for one of its trigger's runs to end.
    case 'held': return `⏳ ${o.reason || t('automation.events.held')}`;
    case 'skipped': return `✗ ${o.reason || t('automation.events.skipped')}`;
    case 'awaiting': return `⏳ ${o.reason || t('automation.events.awaiting')}`;
    case 'failed': return `✗ ${o.reason || t('automation.events.failed')}`;
    case 'unreachable': return `✗ ${o.reason || t('automation.events.unreachable')}`;
  }
}

/** MC-98: whether 시험 실행 has anything to run for a trigger: not when every outcome says its source could not be read. */
export function replayable(row: TriggerRow): boolean {
  return row.outcomes.some((o) => o.outcome !== 'unreachable');
}

/** MC-99: the approvals, the oldest first (the host lists them so; kept here for any order it sends). */
export function approvalsInOrder(approvals: readonly Approval[]): Approval[] {
  return [...approvals].sort((a, b) => a.at.localeCompare(b.at));
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

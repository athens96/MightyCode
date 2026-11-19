// A delegation notice as the human reads it. The parent's run gets the pointer text delegation writes (mighty-delegation
// delivery.rs `notice_text`: "[Mighty Code notice <id>] Your child <child> reported revision <n>. …"); the queue's held rows
// and the conversation show a short chip instead — the child pane's title and what happened — with that text as the tooltip.

export type NoticeOutcome = 'reported' | 'endedWithoutReport' | 'failedToStart';

export interface DelegationNotice {
  /** The child pane's id (a child's id is its pane's). */
  childId: string;
  outcome: NoticeOutcome;
  /** The whole notice as the agent read it. */
  raw: string;
}

const HEAD = /^\[Mighty (?:Code|Claude) notice [^\]\n]+\] Your child (\S+?)('s run ended without a new report| reported revision \d+| could not be started)/;

/** The notice `text` is, or null when it is anything else (a follow-up, a request, a task notice). */
export function parseDelegationNotice(text: string): DelegationNotice | null {
  const trimmed = text.trim();
  if (trimmed.includes('\n')) return null;
  const match = HEAD.exec(trimmed);
  if (!match) return null;
  const said = match[2]!;
  const outcome: NoticeOutcome = said.startsWith("'s") ? 'endedWithoutReport' : said.startsWith(' reported') ? 'reported' : 'failedToStart';
  return { childId: match[1]!, outcome, raw: trimmed };
}

/**
 * A request's words split into the notices that lead it (held notices go first, each its own paragraph, delivery.rs `release`)
 * and the rest, which may be empty when the run was only notices.
 */
export function leadingNotices(text: string): { notices: DelegationNotice[]; rest: string } {
  const parts = text.split('\n\n');
  const notices: DelegationNotice[] = [];
  let index = 0;
  for (; index < parts.length; index++) {
    const notice = parseDelegationNotice(parts[index]!);
    if (!notice) break;
    notices.push(notice);
  }
  return { notices, rest: notices.length === 0 ? text : parts.slice(index).join('\n\n') };
}

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

/** The chip's words after the title, in the language. */
export function noticeWords(outcome: NoticeOutcome, t: Translate): string {
  switch (outcome) {
    case 'reported': return t('queue.notice.reported');
    case 'endedWithoutReport': return t('queue.notice.endedWithoutReport');
    case 'failedToStart': return t('queue.notice.failedToStart');
  }
}

/** The child's pane title, or the generic name once its pane is gone. */
export function noticeTitle(notice: DelegationNotice, sessions: readonly { id: string; title: string }[], t: Translate): string {
  const title = sessions.find((session) => session.id === notice.childId)?.title.trim();
  return title || t('queue.notice.child');
}

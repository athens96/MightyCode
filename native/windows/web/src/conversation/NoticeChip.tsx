import { useServices, useStoreSelector } from '../app/context';
import { Icon } from '../icons/Icon';
import { noticeTitle, noticeWords, type DelegationNotice } from './delegationNotice';

const glyph = { reported: 'clipboard-check', endedWithoutReport: 'circle-stop', failedToStart: 'triangle-alert' } as const;

/** A delegation notice as a chip: the child pane's title and what happened, the notice the agent read as its tooltip. */
export function NoticeChip({ notice }: { notice: DelegationNotice }) {
  const { t } = useServices();
  const childTitle = useStoreSelector(
    (state) => state.snapshot?.sessions.find((session) => session.id === notice.childId)?.title,
  );
  const title = noticeTitle(notice, childTitle !== undefined ? [{ id: notice.childId, title: childTitle }] : [], t);
  const words = noticeWords(notice.outcome, t);
  return (
    <span className={'notice-chip outcome-' + notice.outcome} title={notice.raw} data-notice-child={notice.childId}>
      <Icon name={glyph[notice.outcome]} size={12} weight={2} className="notice-chip-mark" />
      <span className="notice-chip-title">{title}</span>
      <span className="notice-chip-words">{words}</span>
    </span>
  );
}

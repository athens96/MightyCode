import { describe, expect, it } from 'vitest';
import { leadingNotices, noticeTitle, noticeWords, parseDelegationNotice } from './delegationNotice';

// The three notice texts mighty-delegation delivery.rs `notice_text` writes (its tests/delivery.rs holds them).
const reported = '[Mighty Code notice n-1] Your child c-1 reported revision 2. Read the report with child_status; merge it with merge when it is right.';
const ended = "[Mighty Code notice n-2] Your child c-2's run ended without a new report; its report revision is still 2. Call child_status to see where it stands.";
const failed = '[Mighty Code notice n-3] Your child c-3 could not be started. Only a human can discard it, from its card.';
const t = (key: string) => key;

describe('delegation notices', () => {
  it('reads the child and what happened from each notice text', () => {
    expect(parseDelegationNotice(reported)).toEqual({ childId: 'c-1', outcome: 'reported', raw: reported });
    expect(parseDelegationNotice(ended)).toMatchObject({ childId: 'c-2', outcome: 'endedWithoutReport' });
    expect(parseDelegationNotice(failed)).toMatchObject({ childId: 'c-3', outcome: 'failedToStart' });
    expect(parseDelegationNotice('[Mighty Claude notice x] Your child 8b1e5d93-2c47 reported revision 1.')).toMatchObject({ childId: '8b1e5d93-2c47' });
  });

  it('leaves anything else as it is', () => {
    expect(parseDelegationNotice('fix the login page')).toBeNull();
    expect(parseDelegationNotice('[Mighty Code task notice] the tests finished')).toBeNull();
    expect(parseDelegationNotice('[Mighty Code follow-up f-1] Your parent sent you this follow-up:\n\nmore\n\nWhen you are done…')).toBeNull();
  });

  it('splits the notices that lead a request from its words', () => {
    expect(leadingNotices(`${reported}\n\n${ended}\n\nnow merge them`)).toMatchObject({ notices: [{ childId: 'c-1' }, { childId: 'c-2' }], rest: 'now merge them' });
    expect(leadingNotices(reported)).toMatchObject({ notices: [{ childId: 'c-1' }], rest: '' });
    expect(leadingNotices('plain\n\nwords')).toEqual({ notices: [], rest: 'plain\n\nwords' });
  });

  it("names the child by its pane's title, or generically once it is gone", () => {
    const notice = parseDelegationNotice(reported)!;
    expect(noticeTitle(notice, [{ id: 'c-1', title: '장바구니 테스트' }], t)).toBe('장바구니 테스트');
    expect(noticeTitle(notice, [], t)).toBe('queue.notice.child');
    expect(noticeWords('reported', t)).toBe('queue.notice.reported');
    expect(noticeWords('failedToStart', t)).toBe('queue.notice.failedToStart');
  });
});

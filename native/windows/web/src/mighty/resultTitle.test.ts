// The result head in 기록 reads the host's title (Core MightyGraphBlockModel.ResultTitle), 최종 결과 without one.
import { describe, expect, it } from 'vitest';
import ko from '@locales/ko.json';
import { translator } from '../i18n/locale';
import { timelineResultTitle } from './resultTitle';

const t = translator([ko as Record<string, string>]);
const awaiting = t('graph.block.awaitingNotice');
const soFar = t('graph.block.resultSoFar', { count: 2 });

describe('result titles', () => {
  it('read the host title of a 기록 group, 최종 결과 without one', () => {
    expect(awaiting).toBe('알림 대기 중');
    expect(timelineResultTitle({ resultTitle: awaiting }, t)).toBe(awaiting);
    expect(timelineResultTitle({}, t)).toBe('최종 결과');
  });

  it('read the result so far from the host', () => {
    expect(soFar).toBe('현재까지의 결과 · 백그라운드 작업 2개 기다리는 중');
    expect(timelineResultTitle({ resultTitle: soFar }, t)).toBe(soFar);
  });
});

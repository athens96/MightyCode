// The result head's title in 기록: the host's (Core MightyGraphBlockModel.ResultTitle), which reads 알림 대기 중 while the
// request awaits the notices of background work it started, else 현재까지의 결과 with the count while background work of its request still
// runs; 최종 결과 from a host that sends none.
import type { Translate } from '../i18n/locale';
import type { TimelineGroup } from './types';

/** A 기록 group's result head. */
export function timelineResultTitle(group: Pick<TimelineGroup, 'resultTitle'>, t: Translate): string {
  return group.resultTitle ?? t('graph.block.result');
}

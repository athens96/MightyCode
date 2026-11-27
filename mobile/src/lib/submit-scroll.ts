// Where a view moves when a request block appears (natural submit B2; pending.json `scrollTargets`, rule `scrollRule`): the
// Mac's SubmitScroll.target, Windows Core's SubmitScroll.Decide and the Windows page's submitScroll. The phone's two bodies,
// the transcript (`basic`) and 흐름 (`diagram`), decide alike. The caller decides `nearBottom` with its own threshold (here
// `lib/follow.ts` FOLLOW_THRESHOLD). Pure.

export type SubmitTarget = 'toRequest' | 'follow' | 'none';

export interface SubmitDecision {
  scroll: SubmitTarget;
}

/**
 * `own`: this composer's send, a queued request that now runs, or 다음 실행. An own request always moves to its block;
 * anything else follows only near the bottom.
 */
export function submitScroll(own: boolean, nearBottom: boolean): SubmitDecision {
  if (own) return { scroll: 'toRequest' };
  return { scroll: nearBottom ? 'follow' : 'none' };
}

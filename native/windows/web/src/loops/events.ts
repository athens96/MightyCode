// MC-119: window events that open/close the loop detail view and the loop create sheet.
import { switchSidebarTab } from '../shell/botTab';

export const openLoopDetailEvent = 'mighty:open-loop-detail';
export const closeLoopDetailEvent = 'mighty:close-loop-detail';
export const openLoopCreateEvent = 'mighty:open-loop-create';

export interface OpenLoopCreateDetail {
  paneId: string;
  goal?: string;
  /** Opened from the pane's composer with its text as the goal: a started loop then clears that text (and only then). */
  fromComposer?: boolean;
}

/** Shows a loop's detail view in the main area (optionally switching the sidebar to the 루프 tab). */
export function openLoopDetail(loopId: string, focusTab = false) {
  if (focusTab) switchSidebarTab('loops');
  window.dispatchEvent(new CustomEvent<string>(openLoopDetailEvent, { detail: loopId }));
}

/** Returns the main area from the loop detail view back to the workspace panes/dashboard. */
export function closeLoopDetail() {
  window.dispatchEvent(new Event(closeLoopDetailEvent));
}

/** Opens the loop create sheet for a pane, prefilled with `goal` (the composer's text when `fromComposer`). */
export function openLoopCreate(paneId: string, goal = '', fromComposer = false) {
  window.dispatchEvent(new CustomEvent<OpenLoopCreateDetail>(openLoopCreateEvent, { detail: { paneId, goal, fromComposer } }));
}

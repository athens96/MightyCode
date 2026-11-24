// MC-68: the sidebar's two tabs, 워크스페이스 and 봇, and what the 봇 tab shows: the bots at work (an independent bot, one with
// instructions of its own or with schedules and triggers) unless every pane is shown, grouped by state (needs you, working, idle,
// asleep), each with one line saying what it is doing. Pure, but for the tab and the show-all choice kept in this window's storage.
import type { PaneInfo, SidebarBot } from './types';

export type SidebarTab = 'workspaces' | 'bots';
export type BotState = 'needsYou' | 'working' | 'idle' | 'asleep';
export const botStates: readonly BotState[] = ['needsYou', 'working', 'idle', 'asleep'];

const tabKey = 'mighty.sidebar.tab';
const allPanesKey = 'mighty.sidebar.botsAllPanes';
/** The event that switches the sidebar's tab (its detail the tab), sent by a pane row's `@name` and the shortcuts. */
export const sidebarTabEvent = 'mighty:sidebar-tab';

export function savedTab(): SidebarTab {
  try { return window.localStorage.getItem(tabKey) === 'bots' ? 'bots' : 'workspaces'; } catch { return 'workspaces'; }
}

export function saveTab(tab: SidebarTab) {
  try { window.localStorage.setItem(tabKey, tab); } catch { /* storage off: this session only */ }
}

export function switchSidebarTab(tab: SidebarTab) {
  window.dispatchEvent(new CustomEvent<SidebarTab>(sidebarTabEvent, { detail: tab }));
}

export function savedAllPanes(): boolean {
  try { return window.localStorage.getItem(allPanesKey) === 'true'; } catch { return false; }
}

export function saveAllPanes(all: boolean) {
  try { window.localStorage.setItem(allPanesKey, String(all)); } catch { /* storage off: this session only */ }
}

/** A bot at work: an independent one, one with instructions of its own, or one with schedules or triggers sent to it. */
export function atWork(bot: SidebarBot): boolean {
  return !bot.workspaceId || bot.instructed === true || (bot.jobs ?? 0) > 0;
}

/** Where a bot stands: waiting for the user first (whatever else is true), then running, asleep, else idle. */
export function botState(bot: SidebarBot, info: PaneInfo | undefined): BotState {
  if (info?.attention || info?.tone === 'wait') return 'needsYou';
  if (info?.tone === 'run') return 'working';
  if (bot.asleep) return 'asleep';
  return 'idle';
}

/** The bots the 봇 tab lists (every bot when every pane is shown, else those at work and any waiting for the user) and how many are left out. */
export function listedBots(bots: readonly SidebarBot[], panes: ReadonlyMap<string, PaneInfo>, allPanes: boolean): { listed: SidebarBot[]; hidden: number } {
  if (allPanes) return { listed: [...bots], hidden: 0 };
  const listed = bots.filter((bot) => atWork(bot) || botState(bot, panes.get(bot.id)) === 'needsYou');
  return { listed, hidden: bots.length - listed.length };
}

/** The bots in each state, in the order given (states with none left out). */
export function groupByState(bots: readonly SidebarBot[], panes: ReadonlyMap<string, PaneInfo>): { state: BotState; bots: SidebarBot[] }[] {
  return botStates
    .map((state) => ({ state, bots: bots.filter((bot) => botState(bot, panes.get(bot.id)) === state) }))
    .filter((group) => group.bots.length > 0);
}

/** How many bots wait for the user and how many run: the 봇 tab's badge. */
export function botCounts(bots: readonly SidebarBot[], panes: ReadonlyMap<string, PaneInfo>): { needsYou: number; working: number } {
  let needsYou = 0;
  let working = 0;
  for (const bot of bots) {
    const state = botState(bot, panes.get(bot.id));
    if (state === 'needsYou') needsYou += 1;
    else if (state === 'working') working += 1;
  }
  return { needsYou, working };
}

/** Whether a bot matches the sidebar's search: its name, its workspace or its pane's title. */
export function botMatches(bot: SidebarBot, info: PaneInfo | undefined, search: string): boolean {
  const words = search.trim().toLowerCase();
  if (words.length === 0) return true;
  return [bot.handle, bot.workspace ?? '', info?.title ?? ''].some((text) => text.toLowerCase().includes(words));
}

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

/**
 * The one line under a bot's name saying what it is doing: why it waits for the user, what it is on, that it sleeps, its next run,
 * else where it works (its pane's title, or 독립).
 */
export function statusLine(bot: SidebarBot, info: PaneInfo | undefined, next: string | null | undefined, t: Translate): string {
  switch (botState(bot, info)) {
    case 'needsYou': return info?.attention?.text || info?.wait || t('bot.sidebar.line.needsYou');
    case 'working': return info?.activity || t('bot.sidebar.line.working');
    case 'asleep': return t('bot.sidebar.asleep');
    case 'idle':
      if (next) return t('bot.sidebar.nextRun', { at: next });
      return bot.workspaceId ? info?.title ?? '' : t('bot.sidebar.independent');
  }
}

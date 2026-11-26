// MC-68: the sidebar's two tabs, 워크스페이스 and 봇, and what the 봇 tab shows: every bot (MC-89: no "at work only" view), grouped by
// kind (MC-88) or by state (needs you, working, idle, asleep), each with one line saying what it is doing. Pure (MC-100: the app opens on the
// 워크스페이스 tab every time).
import type { PaneInfo, SidebarBot, SidebarItem } from './types';

export type SidebarTab = 'workspaces' | 'bots';
export type BotState = 'needsYou' | 'working' | 'idle' | 'asleep';
export const botStates: readonly BotState[] = ['needsYou', 'working', 'idle', 'asleep'];

/** The event that switches the sidebar's tab (its detail the tab), sent by a pane row's `@name` and the shortcuts. */
export const sidebarTabEvent = 'mighty:sidebar-tab';

export function switchSidebarTab(tab: SidebarTab) {
  window.dispatchEvent(new CustomEvent<SidebarTab>(sidebarTabEvent, { detail: tab }));
}

/** A bot at work: an independent one, one with instructions of its own, or one with schedules or triggers sent to it. */
export function atWork(bot: SidebarBot): boolean {
  return !bot.workspaceId || bot.instructed === true || (bot.jobs ?? 0) > 0;
}

/**
 * MC-75: a workspace's rows without its bots at work, which the 봇 tab lists: a bot made to work in a workspace runs there as a pane,
 * but it is shown in the 봇 tab only. One with delegated children under it stays, so they keep their parent.
 */
export function workspaceItems(items: readonly SidebarItem[], bots: readonly SidebarBot[]): SidebarItem[] {
  const working = new Set(bots.filter(atWork).map((bot) => bot.id));
  if (working.size === 0) return [...items];
  return items.filter((item, i) => !(item.type === 'pane' && item.depth === 0 && working.has(item.id) && !((items[i + 1]?.depth ?? 0) > 0)));
}

/** Where a bot stands: waiting for the user first (whatever else is true), then running, asleep, else idle. */
export function botState(bot: SidebarBot, info: PaneInfo | undefined): BotState {
  if (info?.attention || info?.tone === 'wait') return 'needsYou';
  if (info?.tone === 'run') return 'working';
  if (bot.asleep) return 'asleep';
  return 'idle';
}

/** The bots in each state, in the order given (states with none left out). */
export function groupByState(bots: readonly SidebarBot[], panes: ReadonlyMap<string, PaneInfo>): { state: BotState; bots: SidebarBot[] }[] {
  return botStates
    .map((state) => ({ state, bots: bots.filter((bot) => botState(bot, panes.get(bot.id)) === state) }))
    .filter((group) => group.bots.length > 0);
}

/**
 * MC-88: the 봇 tab's default grouping, by kind: the independent bots first, then the workspace bots, each group in state order (needs
 * you, working, idle, asleep) so a bot waiting for the user stands at the top of its group. Empty groups are left out.
 */
export function groupByKind(bots: readonly SidebarBot[], panes: ReadonlyMap<string, PaneInfo>): { kind: 'independent' | 'workspace'; bots: SidebarBot[] }[] {
  const inStateOrder = (kind: readonly SidebarBot[]) => groupByState(kind, panes).flatMap((group) => group.bots);
  return [
    { kind: 'independent' as const, bots: inStateOrder(bots.filter((bot) => !bot.workspaceId)) },
    { kind: 'workspace' as const, bots: inStateOrder(bots.filter((bot) => bot.workspaceId)) },
  ].filter((group) => group.bots.length > 0);
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

/** Whether a bot matches the sidebar's search: its name, its workspace, what it watches (MC-108) or its pane's title. */
export function botMatches(bot: SidebarBot, info: PaneInfo | undefined, search: string): boolean {
  const words = search.trim().toLowerCase();
  if (words.length === 0) return true;
  return [bot.handle, bot.workspace ?? '', ...(bot.watches ?? []).map((w) => w.label), info?.title ?? ''].some((text) => text.toLowerCase().includes(words));
}

/** MC-108: what an independent bot watches, in words (`MightyCode, owner/name`); '' for a workspace bot or one watching nothing. */
export function watchWords(bot: SidebarBot): string {
  return bot.workspaceId ? '' : (bot.watches ?? []).map((w) => w.label).join(', ');
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

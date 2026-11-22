// MC-28: a forked pane listed under the pane it was forked from: right after its source (and the source's own children and forks),
// one step further in. Only top-level panes of the same workspace move; a fork whose source is closed or elsewhere stays where it is.
import type { PaneInfo, SidebarItem } from './types';

export interface NestedItem {
  item: SidebarItem;
  /** Steps further in than the item's own depth. */
  indent: number;
}

export function nestForks(items: readonly SidebarItem[], panes: ReadonlyMap<string, PaneInfo>): NestedItem[] {
  const topIds = new Set(items.filter((i) => i.type === 'pane' && i.depth === 0).map((i) => i.id));
  const sourceOf = (item: SidebarItem): string | undefined => {
    if (item.type !== 'pane' || item.depth !== 0) return undefined;
    const source = panes.get(item.id)?.forkedFrom?.paneId;
    return source && source !== item.id && topIds.has(source) ? source : undefined;
  };
  const forksOf = new Map<string, SidebarItem[]>();
  for (const item of items) {
    const source = sourceOf(item);
    if (source) forksOf.set(source, [...(forksOf.get(source) ?? []), item]);
  }
  if (forksOf.size === 0) return items.map((item) => ({ item, indent: 0 }));
  const out: NestedItem[] = [];
  const placed = new Set<string>();
  const place = (item: SidebarItem, indent: number) => {
    if (placed.has(item.type + ':' + item.id)) return;
    placed.add(item.type + ':' + item.id);
    out.push({ item, indent });
  };
  // A top-level pane's block: itself and the items under it (deeper depth) as they come.
  const blocks: SidebarItem[][] = [];
  for (const item of items) {
    const last = blocks[blocks.length - 1];
    if (item.depth === 0 || !last) blocks.push([item]);
    else last.push(item);
  }
  const key = (item: SidebarItem) => item.type + ':' + item.id;
  const blockOf = new Map(blocks.map((b) => [key(b[0]!), b]));
  const emit = (block: SidebarItem[], indent: number, seen: Set<string>) => {
    for (const item of block) place(item, indent);
    const head = block[0];
    if (!head || seen.has(head.id)) return;
    const next = new Set(seen).add(head.id);
    for (const fork of forksOf.get(head.id) ?? []) {
      const forkBlock = blockOf.get(key(fork));
      if (forkBlock) emit(forkBlock, Math.min(indent + 1, 4), next);
    }
  };
  for (const block of blocks) {
    const head = block[0];
    if (!head || sourceOf(head)) continue;
    emit(block, 0, new Set());
  }
  // A cycle of forks (each the other's source) has no root: its panes keep their places.
  for (const block of blocks) for (const item of block) place(item, 0);
  return out;
}

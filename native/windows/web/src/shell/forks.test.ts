import { describe, expect, it } from 'vitest';
import { nestForks } from './forks';
import type { PaneInfo, SidebarItem } from './types';

const pane = (id: string, depth = 0): SidebarItem => ({ type: 'pane', id, depth });
const info = (id: string, source?: string): PaneInfo => ({ id, forkedFrom: source ? { paneId: source, title: 'src' } : undefined } as unknown as PaneInfo);

describe('nestForks', () => {
  it('puts a fork right under its source, one step in', () => {
    const items = [pane('a'), pane('b'), pane('c')];
    const panes = new Map([['a', info('a')], ['b', info('b')], ['c', info('c', 'a')]]);
    expect(nestForks(items, panes).map((n) => [n.item.id, n.indent])).toEqual([['a', 0], ['c', 1], ['b', 0]]);
  });

  it('keeps a source block with its children together and nests forks of forks', () => {
    const items = [pane('a'), pane('a-child', 1), pane('b'), pane('f1'), pane('f2')];
    const panes = new Map([['a', info('a')], ['b', info('b')], ['f1', info('f1', 'a')], ['f2', info('f2', 'f1')]]);
    expect(nestForks(items, panes).map((n) => [n.item.id, n.indent])).toEqual([['a', 0], ['a-child', 0], ['f1', 1], ['f2', 2], ['b', 0]]);
  });

  it('leaves a fork whose source is gone where it is, and survives a cycle', () => {
    const items = [pane('x'), pane('y')];
    expect(nestForks(items, new Map([['x', info('x', 'closed')], ['y', info('y')]])).map((n) => n.item.id)).toEqual(['x', 'y']);
    const cycle = nestForks(items, new Map([['x', info('x', 'y')], ['y', info('y', 'x')]]));
    expect(cycle.map((n) => n.item.id).sort()).toEqual(['x', 'y']);
  });
});

import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { Store } from '../store/store';
import { clampRatio, dropZone, layoutGroups, layoutMinimum } from './geometry';
import { Layout, nextTablistIndex, splitKeyAction } from './Layout';
import type { PaneLayoutNode } from '../store/models';

const tabs = (id: string, ids: string[]): PaneLayoutNode => ({ id, kind: 'tabs', sessionIds: ids, selectedSessionId: ids[0], ratio: 0.5, children: [] });

describe('layout geometry (PaneLayout.cs)', () => {
  it('drop zone: the top strip merges, the nearest edge within 24% splits', () => {
    expect(dropZone(5, 10, 400, 300)).toBe('center');
    expect(dropZone(20, 150, 400, 300)).toBe('left');
    expect(dropZone(390, 150, 400, 300)).toBe('right');
    expect(dropZone(200, 290, 400, 300)).toBe('bottom');
    expect(dropZone(200, 60, 400, 300)).toBe('top');
    expect(dropZone(200, 150, 400, 300)).toBe('center');
    expect(dropZone(10, 100, 0, 300)).toBe('center');
  });
  it('ratio is kept within 0.15 to 0.85', () => {
    expect(clampRatio(0.02)).toBe(0.15);
    expect(clampRatio(0.9)).toBe(0.85);
    expect(clampRatio(Number.NaN)).toBe(0.5);
  });
  it('minimum sums along the split axis with the divider', () => {
    const split: PaneLayoutNode = { id: 's', kind: 'split', axis: 'horizontal', ratio: 0.5, sessionIds: [], children: [tabs('a', ['1']), tabs('b', ['2'])] };
    expect(layoutMinimum(split)).toEqual({ width: 315 * 2 + 6, height: 290 });
    expect(layoutGroups(split).map((g) => g.id)).toEqual(['a', 'b']);
  });

  it('cycles tablist focus and adjusts/resets split ratio via keyboard', () => {
    expect(nextTablistIndex(3, 0, 'ArrowRight')).toBe(1);
    expect(nextTablistIndex(3, 2, 'ArrowRight')).toBe(0);
    expect(nextTablistIndex(3, 0, 'ArrowLeft')).toBe(2);
    expect(nextTablistIndex(3, 2, 'Home')).toBe(0);
    expect(nextTablistIndex(3, 0, 'End')).toBe(2);
    expect(nextTablistIndex(3, 0, 'Enter')).toBeNull();

    expect(splitKeyAction(0.5, 'ArrowLeft')).toEqual({ kind: 'resize', ratio: 0.45 });
    expect(splitKeyAction(0.5, 'ArrowUp')).toEqual({ kind: 'resize', ratio: 0.45 });
    expect(splitKeyAction(0.5, 'ArrowRight')).toEqual({ kind: 'resize', ratio: 0.55 });
    expect(splitKeyAction(0.5, 'ArrowDown')).toEqual({ kind: 'resize', ratio: 0.55 });
    expect(splitKeyAction(0.16, 'ArrowLeft')).toEqual({ kind: 'resize', ratio: 0.15 });
    expect(splitKeyAction(0.5, 'Home')).toEqual({ kind: 'reset' });
    expect(splitKeyAction(0.5, 'End')).toEqual({ kind: 'reset' });
    expect(splitKeyAction(0.5, '0')).toEqual({ kind: 'reset' });
    expect(splitKeyAction(0.5, 'Tab')).toBeNull();
  });

  it('renders focusable split separator with ARIA value range attributes', () => {
    const split: PaneLayoutNode = {
      id: 's1',
      kind: 'split',
      axis: 'horizontal',
      ratio: 0.35,
      sessionIds: [],
      children: [tabs('a', ['1']), tabs('b', ['2'])],
    };
    const store = new Store();
    const services = {
      bridge: { call: () => Promise.resolve(null) },
      store,
      t: translator([{}]),
    } as unknown as AppServices;
    const html = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(Layout, { layout: { workspaceId: 'w1', mode: 'grid', root: split } }),
      ),
    );
    expect(html).toContain('role="separator"');
    expect(html).toContain('tabindex="0"');
    expect(html).toContain('aria-valuenow="35"');
    expect(html).toContain('aria-valuemin="10"');
    expect(html).toContain('aria-valuemax="90"');
  });
});

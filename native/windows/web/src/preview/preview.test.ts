import { describe, expect, it } from 'vitest';
import { reduce, emptyState } from '../store/store';
import { firstPerPane, previewMenu, reducePreview, shortAddress, type PreviewInfo, type PreviewMessage, type PreviewServer } from './preview';

const t = (key: string, vars?: Record<string, string | number>) => key + (vars ? ' ' + JSON.stringify(vars) : '');

const info: PreviewInfo = { url: 'http://localhost:41000/', title: 'App', browser: 'Google Chrome', picking: false, picked: null, consoleErrors: 2, failedRequests: 0 };
const message = (extra: Partial<PreviewMessage> = {}): PreviewMessage => ({ kind: 'preview', paneId: 'p1', preview: info, note: null, ...extra });

const server = (paneId: string, port: number, extra: Partial<PreviewServer> = {}): PreviewServer =>
  ({ paneId, branch: null, url: `http://localhost:${port}/`, port, action: null, open: false, ...extra });

describe('the live preview (MC-7)', () => {
  it('keeps a pane while its preview is open or something is said on it, and drops it after', () => {
    let record = reducePreview(undefined, message());
    expect(record?.p1).toEqual({ preview: info, note: null });
    record = reducePreview(record, message({ preview: null, note: 'blocked' }));
    expect(record?.p1).toEqual({ preview: null, note: 'blocked' });
    record = reducePreview(record, message({ preview: null, note: null }));
    expect(record).toEqual({});
    const same = {};
    expect(reducePreview(same, message({ paneId: 'other', preview: null }))).toBe(same);
  });

  it('is a store message the host pushes', () => {
    const state = reduce(emptyState, message());
    expect(state.preview?.p1?.preview?.consoleErrors).toBe(2);
    expect(reduce(state, message({ preview: null })).preview).toEqual({});
  });

  it('lists the servers of each place, the run action named, and opens all once per pane', () => {
    const rows = previewMenu(t, {
      available: true,
      browser: 'Google Chrome',
      servers: [
        server('main', 5173, { action: 'dev' }),
        server('main', 41001),
        server('wt', 41010, { branch: 'feature/login', open: true }),
      ],
      outside: [{ paneId: 'wt', branch: 'feature/login', port: 3000 }],
    });
    const labels = rows.map((row) => row.label);
    expect(labels[0]).toBe('preview.menu.action {"place":"preview.menu.main","action":"dev","address":"localhost:5173"}');
    expect(labels[1]).toBe('preview.menu.server {"place":"preview.menu.main","address":"localhost:41001"}');
    expect(labels[2]).toBe('● preview.menu.server {"place":"feature/login","address":"localhost:41010"}');
    expect(rows[2]?.open).toEqual({ paneId: 'wt', url: 'http://localhost:41010/', external: false });
    expect(rows.some((row) => row.label === 'preview.menu.openAll {"count":"2"}' && !row.disabled)).toBe(true);
    const outside = rows.find((row) => row.label.startsWith('preview.menu.outside'));
    expect(outside).toMatchObject({ disabled: true, open: null });
    expect(outside?.label).toContain('"port":"3000"');
  });

  it('says why when there is nothing to open, and opens in the default browser without one to drive', () => {
    expect(previewMenu(t, { available: false, reason: 'SSH not yet' })).toEqual([{ label: 'SSH not yet', open: null, disabled: true }]);
    expect(previewMenu(t, { available: true, browser: 'Edge', servers: [] }).map((r) => r.label)).toEqual(['preview.menu.none']);
    const rows = previewMenu(t, { available: true, browser: null, servers: [server('p1', 41000)] });
    expect(rows[0]).toEqual({ label: 'preview.menu.external', open: null, disabled: true });
    expect(rows[1]?.open).toEqual({ paneId: 'p1', url: 'http://localhost:41000/', external: true });
    expect(rows.some((row) => row.label.startsWith('preview.menu.openAll'))).toBe(false);
  });

  it('takes the first server of each pane and shows an address without its scheme', () => {
    expect(firstPerPane([server('a', 1), server('a', 2), server('b', 3)]).map((s) => s.port)).toEqual([1, 3]);
    expect(shortAddress('https://localhost:41000/settings')).toBe('localhost:41000/settings');
  });
});

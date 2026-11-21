import { afterEach, describe, expect, it } from 'vitest';
import { emptyState, type StoreState } from '../store/store';
import type { ShellModel } from '../shell/types';
import { goneReport, pageCovers, sameReport, SlotReporter, slotReport, wantsSnapshot, type SlotReport } from './bounds';
import { applyHostPush, hostCapabilities, isHostPush, nativeTerminal, readCapabilities, resetHostCapabilities, subscribeHost } from './host';
import { safeSnapshotUrl } from './NativeSlot';

const box = (left: number, top: number, width: number, height: number) => ({ left, top, width, height, right: left + width, bottom: top + height });

describe('host push', () => {
  afterEach(resetHostCapabilities);

  it('keeps what the host draws natively and reads anything else as the page’s', () => {
    expect(isHostPush({ kind: 'host' })).toBe(true);
    expect(isHostPush({ kind: 'shell' })).toBe(false);
    expect(readCapabilities({ platform: 'macos', surfaces: { terminal: 'native' } })).toEqual({ platform: 'macos', surfaces: { terminal: 'native' } });
    // A kind of view the page no longer has (an older host's browser) is not kept.
    expect(readCapabilities({ platform: 'macos', surfaces: { terminal: 'native', browser: 'native' } })).toEqual({ platform: 'macos', surfaces: { terminal: 'native' } });
    expect(readCapabilities({ platform: 7, surfaces: { terminal: 'ghostty' } })).toEqual({ platform: '', surfaces: { terminal: 'web' } });
    expect(readCapabilities({})).toEqual({ platform: '', surfaces: { terminal: 'web' } });
  });

  it('is absent until the Rust host sends it, then tells its listeners', () => {
    expect(hostCapabilities()).toBeUndefined();
    expect(nativeTerminal()).toBe(false);
    let told = 0;
    const stop = subscribeHost(() => { told += 1; });
    applyHostPush({ kind: 'host', platform: 'windows', surfaces: { terminal: 'web' } });
    expect(hostCapabilities()?.platform).toBe('windows');
    expect(nativeTerminal()).toBe(false);
    applyHostPush({ kind: 'host', platform: 'macos', surfaces: { terminal: 'native' } });
    expect(nativeTerminal()).toBe(true);
    stop();
    applyHostPush({ kind: 'host', platform: 'windows', surfaces: { terminal: 'web' } });
    expect(told).toBe(2);
  });
});

describe('slot bounds', () => {
  it('reports device pixels covering the slot, hidden when empty or not shown', () => {
    expect(slotReport(box(10.4, 20, 100, 50), 1.5, true, false)).toEqual({ rect: { x: 16, y: 30, width: 150, height: 75 }, visible: true, occluded: false });
    expect(slotReport(box(0, 0, 0, 50), 1, true, false).visible).toBe(false);
    expect(slotReport(box(0, 0, 10, 10), 1, false, false).visible).toBe(false);
    expect(slotReport(box(0, 0, 10, 10), 2, true, true)).toEqual({ rect: { x: 0, y: 0, width: 20, height: 20 }, visible: true, occluded: true });
  });

  it('compares every field', () => {
    const a = slotReport(box(1, 1, 1, 1), 1, true, false);
    expect(sameReport(a, { ...a, rect: { ...a.rect } })).toBe(true);
    expect(sameReport(undefined, a)).toBe(false);
    expect(sameReport(a, { ...a, occluded: true })).toBe(false);
    expect(sameReport(a, { ...a, rect: { ...a.rect, width: 2 } })).toBe(false);
  });
});

describe('slot occlusion', () => {
  const state = (patch: Partial<StoreState>): StoreState => ({ ...emptyState, ...patch });
  const slot = box(100, 100, 200, 200);
  const none = () => [];

  it('is covered by any dialog, host view or frame dialog, wherever it stands', () => {
    expect(pageCovers(state({}), slot, none)).toBe(false);
    expect(pageCovers(state({ dialogs: [{ id: 1, dialog: { type: 'confirm', title: '', message: '', close: '', defaultPrimary: false } }] }), slot, none)).toBe(true);
    expect(pageCovers(state({ natives: [{ id: 1, place: 'dialog', view: null }] as unknown as StoreState['natives'] }), slot, none)).toBe(true);
    expect(pageCovers(state({ shell: { dialogOpen: true } as ShellModel }), slot, none)).toBe(true);
  });

  it('is covered by a page overlay only where one overlaps it', () => {
    expect(pageCovers(state({ overlays: 1 }), slot, () => [box(0, 0, 50, 50)])).toBe(false);
    expect(pageCovers(state({ overlays: 1 }), slot, () => [box(250, 250, 100, 100)])).toBe(true);
    // An overlay counted closed is not measured at all.
    expect(pageCovers(state({ overlays: 0 }), slot, () => { throw new Error('measured'); })).toBe(false);
  });
});

describe('slot reporter', () => {
  const shown = (x = 10): SlotReport => ({ rect: { x, y: 20, width: 300, height: 200 }, visible: true, occluded: false });

  function harness(snapshot: string | null = 'data:image/png;base64,AAAA') {
    const calls: { method: string; args: Record<string, unknown> }[] = [];
    const pictures: (string | undefined)[] = [];
    let answer: (value: unknown) => void = () => {};
    const call = (method: string, args: object) => {
      calls.push({ method, args: args as Record<string, unknown> });
      if (method === 'surfaceAction') return new Promise((resolve) => { answer = () => resolve(snapshot); });
      return Promise.resolve(null);
    };
    const reporter = new SlotReporter('p1', 'terminal', call, (url) => pictures.push(url));
    return { calls, pictures, reporter, answer: () => answer(snapshot) };
  }

  it('sends surfaceBounds only when the slot changed', () => {
    const { calls, reporter } = harness();
    reporter.update(shown());
    reporter.update(shown());
    reporter.update(shown(12));
    expect(calls).toEqual([
      { method: 'surfaceBounds', args: { paneId: 'p1', surface: 'terminal', ...shown() } },
      { method: 'surfaceBounds', args: { paneId: 'p1', surface: 'terminal', ...shown(12) } },
    ]);
  });

  it('asks for the picture before the view steps aside, and shows it while the slot stays covered', async () => {
    const { calls, pictures, reporter, answer } = harness();
    reporter.update(shown());
    reporter.update({ ...shown(), occluded: true });
    expect(calls.map((c) => c.method)).toEqual(['surfaceBounds', 'surfaceAction', 'surfaceBounds']);
    expect(calls[1]!.args).toEqual({ paneId: 'p1', surface: 'terminal', action: 'snapshot' });
    expect(calls[2]!.args.occluded).toBe(true);
    answer();
    await Promise.resolve();
    await Promise.resolve();
    expect(pictures.at(-1)).toBe('data:image/png;base64,AAAA');
    reporter.update(shown());
    expect(pictures.at(-1)).toBeUndefined();
  });

  it('drops a picture that comes after the slot shows again, and asks none for a hidden pane', async () => {
    const { calls, pictures, reporter, answer } = harness();
    reporter.update(shown());
    reporter.update({ ...shown(), occluded: true });
    reporter.update(shown());
    answer();
    await Promise.resolve();
    await Promise.resolve();
    expect(pictures.filter((p) => p !== undefined)).toEqual([]);
    const before = calls.length;
    reporter.update({ ...shown(), visible: false });
    reporter.update({ ...shown(), visible: false, occluded: true });
    expect(calls.slice(before).map((c) => c.method)).toEqual(['surfaceBounds', 'surfaceBounds']);
    expect(wantsSnapshot(undefined, { ...shown(), occluded: true })).toBe(false);
  });

  it('takes no picture a platform does not give', async () => {
    const { pictures, reporter, answer } = harness(null);
    reporter.update(shown());
    reporter.update({ ...shown(), occluded: true });
    answer();
    await Promise.resolve();
    await Promise.resolve();
    expect(pictures.filter((p) => p !== undefined)).toEqual([]);
  });

  it('hides the view when the slot goes, and reports again from scratch after', () => {
    const { calls, reporter } = harness();
    reporter.update(shown());
    reporter.release();
    expect(calls.at(-1)).toEqual({ method: 'surfaceBounds', args: { paneId: 'p1', surface: 'terminal', ...goneReport } });
    reporter.update(shown());
    expect(calls.at(-1)?.args.visible).toBe(true);
  });
});

describe('safeSnapshotUrl', () => {
  it('accepts base64 PNG, JPEG, and WebP data URLs', () => {
    expect(safeSnapshotUrl('data:image/png;base64,iVBORw0KGgo=')).toBe('data:image/png;base64,iVBORw0KGgo=');
    expect(safeSnapshotUrl('data:image/jpeg;base64,/9j/4AAQSkZJRg==')).toBe('data:image/jpeg;base64,/9j/4AAQSkZJRg==');
    expect(safeSnapshotUrl('data:image/webp;base64,UklGRh4AAABXRUJQ')).toBe('data:image/webp;base64,UklGRh4AAABXRUJQ');
  });

  it('rejects non-image schemes, SVG, empty payloads, and CSS breakout characters', () => {
    expect(safeSnapshotUrl(undefined)).toBeNull();
    expect(safeSnapshotUrl('')).toBeNull();
    expect(safeSnapshotUrl('data:image/png;base64,')).toBeNull();
    expect(safeSnapshotUrl('data:image/svg+xml;base64,PHN2Zz4=')).toBeNull();
    expect(safeSnapshotUrl('https://example.com/snap.png')).toBeNull();
    expect(safeSnapshotUrl('javascript:alert(1)')).toBeNull();
    expect(safeSnapshotUrl('data:image/png;base64,AAAA");background:red;url("')).toBeNull();
    expect(safeSnapshotUrl("data:image/png;base64,AAAA'")).toBeNull();
    expect(safeSnapshotUrl('data:image/png;base64,AAAA)')).toBeNull();
    expect(safeSnapshotUrl('data:image/png;base64,AAAA\nBBBB')).toBeNull();
    expect(safeSnapshotUrl('data:image/png;base64,AAAA\rBBBB')).toBeNull();
  });
});

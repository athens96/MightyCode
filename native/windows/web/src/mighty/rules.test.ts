// The Windows-only layout mode (splitsExecution, which the shared vectors do not lay out) and the rules around the
// layout: sizing and drags, follow, zoom and wheel, dot grid, activity outline, saved sizes. Expected values are the C#
// rules worked by hand.
import { describe, expect, it } from 'vitest';
import type { MightyGraphRun } from '../store/models';
import { PendingNodeID, ResultHeadHeight, emptyResultIds, imagesHeight, makeLayout, nodeId, resultCap, resultDrag, resultFitSize, resultSize, resultViewportLimit, runOf, suffixOf } from './layout';
import {
  arrivedResult, resultCamera, cameraOffset, centredX, progressSafeLeft, ProgressSafeLeft,
  DotsMargin, FollowWatch, WheelGestureGapMs, ZoomMax, activityPhase, barHeight, cameraTween, dashFitting, dashForCard, dotFirst, dotRadius, dotStep, dotsShift, dotsShown, followTargetID, isAtNewest, outlineOf,
  perimeter, runsChanged, scrollsBy, wheelPan, wheelRoute, wheelScrollTarget, zoomAnchored, zoomIn, zoomLabel, zoomOut,
} from './camera';
import { normalizeForRuns, setBlockSize, splittingExecution } from './prefs';

const run = (id: string, status: string, agents: MightyGraphRun['agents'] = []): MightyGraphRun =>
  ({ id, input: 'Request', status, rootEntries: [], agents, resultEntries: [] });
const agent = (id: string, status: string, parentID?: string): MightyGraphRun['agents'][number] =>
  ({ id, title: id, input: 'Work', status, entries: [], ...(parentID ? { parentID } : {}) });
const frame = (layout: ReturnType<typeof makeLayout>, id: string) => layout.nodes.find((n) => n.id === id)?.frame;

describe('layout with the 실행 block (splitsExecution)', () => {
  it('stacks request, 실행 and result, then the draft', () => {
    const layout = makeLayout([run('r1', 'completed')], '', false, new Set(), { splitsExecution: true });
    expect(layout.nodes.map((n) => n.kind)).toEqual(['request', 'work', 'result', 'draft']);
    expect(frame(layout, nodeId('r1', 'request'))).toEqual({ x: 24, y: 24, w: 500, h: 140 });
    expect(frame(layout, nodeId('r1', 'execution'))).toEqual({ x: 24, y: 196, w: 500, h: 280 });
    expect(frame(layout, nodeId('r1', 'result'))).toEqual({ x: 24, y: 508, w: 500, h: 200 });
    expect(frame(layout, PendingNodeID)).toEqual({ x: 24, y: 740, w: 500, h: 140 });
    expect(layout.edges).toEqual([
      { source: nodeId('r1', 'request'), target: nodeId('r1', 'execution'), joins: false },
      { source: nodeId('r1', 'execution'), target: nodeId('r1', 'result'), joins: true },
      { source: nodeId('r1', 'result'), target: PendingNodeID, joins: true },
    ]);
  });

  it('hangs agents off the 실행 block, expanded sizes and a running run without result or draft', () => {
    const r = run('r2', 'running', [agent('a', 'running'), agent('b', 'completed', 'a')]);
    const id = nodeId('r2', 'request');
    const layout = makeLayout([r], '', true, new Set([id]), { splitsExecution: true });
    expect(layout.nodes.map((n) => n.kind)).toEqual(['request', 'work', 'agent', 'agent']);
    expect(frame(layout, id)?.h).toBe(280);
    expect(layout.edges[1]).toEqual({ source: nodeId('r2', 'execution'), target: nodeId('r2', 'agent:a'), joins: false });
    expect(frame(layout, nodeId('r2', 'agent:a'))).toEqual({ x: 94, y: 24 + 280 + 32 + 280 + 32, w: 360, h: 240 });
  });

  it('hangs images and executions beside the 실행 block, never moving it', () => {
    const layout = makeLayout([run('r3', 'completed')], '', false, new Set(), {
      splitsExecution: true, galleries: [{ runID: 'r3', step: 'request', count: 7 }], executions: [{ runID: 'r3', key: 'k' }],
    });
    const work = frame(layout, nodeId('r3', 'execution'))!;
    expect(frame(layout, nodeId('r3', 'images:request'))).toEqual({ x: 532, y: work.y, w: 304, h: imagesHeight(7, false) });
    expect(imagesHeight(7, false)).toBe(38 + 24 + 2 * 88 + 8);
    expect(frame(layout, nodeId('r3', 'ouroboros-execution:k'))).toEqual({ x: 844, y: work.y, w: 380, h: 290 });
  });

  it('uses saved sizes and the newest result fits the window', () => {
    const id = nodeId('r4', 'agent:a');
    const layout = makeLayout([run('r4', 'completed', [agent('a', 'completed')])], '', false, new Set(), {
      splitsExecution: true, blockSizes: { [id]: { width: 9999, height: 10 } }, viewport: { w: 1000, h: 700 }, zoom: 1,
    });
    expect(frame(layout, id)).toMatchObject({ w: 1400, h: 140 });
    expect(frame(layout, nodeId('r4', 'result'))).toMatchObject({ w: 952, h: 652 });
    expect(layout.fittedResultID).toBe(nodeId('r4', 'result'));
  });
});

describe('sizing and drags', () => {
  it('fits, limits and caps the newest result', () => {
    expect(resultFitSize({ w: 300, h: 100 }, false)).toEqual({ w: 500, h: 200 });
    expect(resultViewportLimit({ w: 1048, h: 748 }, 2, true)).toEqual({ w: 300, h: 350 });
    expect(resultCap({ w: 900, h: 900 }, { w: 800, h: 1000 })).toEqual({ w: 800, h: 900 });
    expect(resultSize({ w: 800, h: 600 }, 50.2)).toEqual({ w: 800, h: 140 });
    expect(resultSize({ w: 800, h: 600 }, null)).toEqual({ w: 800, h: 600 });
    // MC-105: a long answer grows the block past the window's height.
    expect(resultSize({ w: 800, h: 600 }, 1830.4)).toEqual({ w: 800, h: 1831 });
  });
  it('saves a released drag side by side', () => {
    const limit = { w: 800, h: 600 };
    expect(resultDrag({ w: 900, h: 300 }, true, false, 'live', null, limit, null).live).toEqual({ width: 800, height: 300 });
    expect(resultDrag({ w: 900, h: 300 }, true, false, 'finished', { width: 1000, height: 700 }, limit, null).save).toEqual({ width: 1000, height: 700 });
    expect(resultDrag({ w: 500, h: 300 }, true, true, 'finished', null, limit, { w: 700, h: 500 }).save).toEqual({ width: 500, height: 300 });
    expect(resultDrag({ w: 500, h: 300 }, true, true, 'cancelled', null, limit, null)).toEqual({ live: null, save: null });
  });
  it('reads node ids back', () => {
    const id = nodeId('런-1', 'agent:x');
    expect(id.startsWith('5:')).toBe(true);
    expect(suffixOf(id)).toBe('agent:x');
    expect(runOf(id)).toBe('런-1');
  });
});

describe('a result that comes in', () => {
  it('is shown once, never on the first pass', () => {
    expect(arrivedResult(undefined, '3:r:result')).toBe(false);
    expect(arrivedResult(null, '3:r:result')).toBe(true);
    expect(arrivedResult('2:r:result', '3:r:result')).toBe(true);
    expect(arrivedResult('3:r:result', '3:r:result')).toBe(false);
    expect(arrivedResult('3:r:result', null)).toBe(false);
  });
  it('lands with its bottom over the margin, or its top at the margin when it is taller than the view', () => {
    const viewport = { w: 800, h: 600 };
    const short = resultCamera({ x: 0, y: 1000, w: 360, h: 200 }, viewport, 1);
    expect(600 - (short.y + 1200)).toBe(16);
    const tall = resultCamera({ x: 0, y: 1000, w: 360, h: 900 }, viewport, 1);
    expect(tall.y + 1000).toBe(16);
  });
});

describe('the progress panel’s room', () => {
  it('centres in the view right of the panel while it shows and the rest holds a request, else in the whole view', () => {
    const wide = { w: 1200, h: 600 };
    expect(progressSafeLeft(false, wide, 1)).toBe(0);
    expect(progressSafeLeft(true, wide, 1)).toBe(ProgressSafeLeft);
    expect(ProgressSafeLeft).toBe(8 + 280 + 16);
    // 820 − 304 = 516 is less than a request's 500 and its two 24 margins.
    expect(progressSafeLeft(true, { w: 820, h: 600 }, 1)).toBe(0);
    expect(progressSafeLeft(true, { w: 820, h: 600 }, 0.5)).toBe(ProgressSafeLeft);
    const block = { x: 24, y: 100, w: 500, h: 140 };
    const plain = cameraOffset(block, wide, 1, true);
    const kept = cameraOffset(block, wide, 1, true, false, ProgressSafeLeft);
    expect(plain.x + 24).toBe((1200 - 500) / 2);
    expect(kept.x + 24).toBe(ProgressSafeLeft + (1200 - ProgressSafeLeft - 500) / 2);
    expect(kept.x + 24).toBeGreaterThanOrEqual(ProgressSafeLeft);
    expect(kept.y).toBe(plain.y);
    expect(centredX(wide, 1, ProgressSafeLeft) - centredX(wide, 1)).toBe(ProgressSafeLeft / 2);
    expect(resultCamera(block, wide, 1, ProgressSafeLeft).x).toBe(centredX(wide, 1, ProgressSafeLeft));
  });
});

describe('a result with nothing to show', () => {
  const said = (id: string): MightyGraphRun => ({ ...run(id, 'completed'), finalOutput: 'Done.' });
  const runs = [run('e1', 'completed'), said('e2'), run('e3', 'running')];
  it('is its header alone once settled, never while it waits or answers', () => {
    const empty = emptyResultIds(runs, () => ({ status: 'completed' }));
    expect([...empty]).toEqual([nodeId('e1', 'result')]);
    expect(emptyResultIds(runs, () => ({ status: 'waiting' })).size).toBe(0);
    expect(emptyResultIds(runs, () => ({ status: 'completed', awaiting: {} })).size).toBe(0);
    expect(emptyResultIds(runs, () => undefined, 'e1').size).toBe(0);
    const layout = makeLayout(runs, '', true, new Set(), { splitsExecution: true, emptyResults: empty });
    expect(frame(layout, nodeId('e1', 'result'))?.h).toBe(ResultHeadHeight);
    expect(frame(layout, nodeId('e2', 'result'))?.h).toBe(200);
  });
});

describe('follow', () => {
  const runs = [run('f1', 'completed'), run('f2', 'running', [agent('a', 'running')])];
  const layout = makeLayout(runs, '', true, new Set(), { splitsExecution: true });
  it('targets the newest run’s lowest flow block', () => {
    expect(followTargetID(layout, runs)).toBe(nodeId('f2', 'agent:a'));
    expect(followTargetID(makeLayout([], '', false, new Set()), [])).toBe(PendingNodeID);
  });
  it('keeps the block where the user left it, within the slack', () => {
    const target = { x: 0, y: 0, w: 100, h: 500 };
    expect(isAtNewest(target, { x: 0, y: 0 }, { w: 800, h: 520 }, 1)).toBe(true);
    expect(isAtNewest(target, { x: 0, y: 0 }, { w: 800, h: 440 }, 1)).toBe(false);
    const watch = new FollowWatch();
    expect(watch.observe('a', target, { x: 5, y: 0 }, { w: 800, h: 600 }, 1)).toEqual({ x: 5, y: 84 });
    expect(watch.observe('a', { ...target, h: 520 }, { x: 5, y: 84 }, { w: 800, h: 600 }, 1)).toEqual({ x: 5, y: 64 });
    watch.moved('a', target, { x: 0, y: -1000 }, { w: 800, h: 600 }, 1);
    expect(watch.following).toBe(false);
  });
  it('follows on through a small scroll up and brings a block that came under the newest into view after the move', () => {
    const target = { x: 0, y: 0, w: 100, h: 500 }, viewport = { w: 800, h: 600 };
    const watch = new FollowWatch();
    // Following 'a' with its bottom 100 above the view's: a small scroll up (still ending in view) keeps following, at the new gap.
    watch.moved('a', target, { x: 0, y: 0 }, viewport, 1);
    expect(watch.followedID).toBe('a');
    watch.moved('a', target, { x: 0, y: 30 }, viewport, 1);
    expect(watch.following).toBe(true);
    // A swipe or a drag under way: a layout pass moves nothing and judges nothing, even when a new block 'b' comes far below.
    const below = { x: 0, y: 560, w: 100, h: 300 };
    expect(watch.observe('b', below, { x: 0, y: 30 }, viewport, 1, true)).toBeNull();
    expect(watch.following).toBe(true);
    expect(watch.followedID).toBe('a');
    // Once it is over, the pass brings 'b' into view, its bottom FollowMargin above the view's.
    expect(watch.observe('b', below, { x: 0, y: 30 }, viewport, 1)).toEqual({ x: 0, y: 600 - 16 - 860 });
    // A scroll that leaves the followed block's bottom out of view ends the following; one back to it follows again.
    watch.moved('b', below, { x: 0, y: 400 }, viewport, 1);
    expect(watch.following).toBe(false);
    expect(watch.followedID).toBeNull();
    expect(watch.observe('b', { ...below, h: 400 }, { x: 0, y: 400 }, viewport, 1)).toBeNull();
    watch.moved('b', below, { x: 0, y: -276 }, viewport, 1);
    expect(watch.following).toBe(true);
  });
  it('reads run-id changes', () => {
    expect(runsChanged(null, [], true)).toBe('first');
    expect(runsChanged(['a'], ['a', 'b'], false)).toBe('follow');
    expect(runsChanged(['a', 'b'], ['b'], false)).toBe('trim');
    expect(runsChanged(['a', 'b'], ['b'], true)).toBe('none');
  });
});

describe('zoom, wheel, dots, activity', () => {
  it('zooms in steps and keeps the centre', () => {
    expect(zoomIn(1.45)).toBe(1.5);
    expect(zoomOut(0.5)).toBe(0.5);
    expect(zoomIn(1)).toBe(1.1);
    expect(zoomLabel(1.1)).toBe('110%');
    expect(zoomAnchored({ x: 0, y: 0 }, { w: 200, h: 100 }, 1, 2)).toEqual({ x: -100, y: -50 });
  });
  it('routes the wheel', () => {
    expect(wheelPan(120, true, false)).toEqual({ x: -120, y: 0 });
    expect(wheelPan(-120, false, true)).toEqual({ x: -120, y: 0 });
    expect(wheelPan(-120, false, false)).toEqual({ x: 0, y: -120 });
    expect(wheelScrollTarget('a', 'a', false)).toBe('a');
    expect(wheelScrollTarget('a', 'b', false)).toBeNull();
    expect(wheelScrollTarget(null, 'b', true)).toBe('b');
  });
  it('keeps a wheel gesture on its first route and lets a new one into a block only where it scrolls', () => {
    const yes = () => true, no = () => false;
    // A new gesture over the selected block goes inside when it can scroll that way, else it pans.
    expect(wheelRoute(null, 1000, true, yes)).toEqual({ at: 1000, inner: true });
    expect(wheelRoute(null, 1000, true, no)).toEqual({ at: 1000, inner: false });
    expect(wheelRoute(null, 1000, false, yes)).toEqual({ at: 1000, inner: false });
    // The gesture's later events (and its glide) keep the route, whatever is under the pointer now, without asking again.
    const asked = () => { throw new Error('asked in mid gesture'); };
    expect(wheelRoute({ at: 1000, inner: false }, 1016, true, asked)).toEqual({ at: 1016, inner: false });
    expect(wheelRoute({ at: 1000, inner: true }, 1000 + WheelGestureGapMs - 1, false, asked)).toEqual({ at: 1000 + WheelGestureGapMs - 1, inner: true });
    // After a pause it is a new gesture: a block scrolled to its end lets the diagram pan.
    expect(wheelRoute({ at: 1000, inner: true }, 1000 + WheelGestureGapMs, true, no)).toEqual({ at: 1000 + WheelGestureGapMs, inner: false });
    // Up at the top and down at the bottom do not scroll; a box that fits never does.
    expect(scrollsBy(0, 200, 800, -30)).toBe(false);
    expect(scrollsBy(0, 200, 800, 30)).toBe(true);
    expect(scrollsBy(600, 200, 800, 30)).toBe(false);
    expect(scrollsBy(600, 200, 800, -30)).toBe(true);
    expect(scrollsBy(0, 200, 200.5, 30)).toBe(false);
    expect(scrollsBy(10, 200, 800, 0)).toBe(false);
  });
  it('lays the dot grid', () => {
    expect(dotsShown(0.3)).toBe(false);
    expect(dotsShown(0.5)).toBe(true);
    expect(dotRadius(0.5)).toBe(0.6);
    expect(dotFirst(-5, 18)).toBe(13);
  });
  it('shifts the dots layer to where the camera puts the dots, by less than a step', () => {
    for (const zoom of [0.5, 1, 1.5]) {
      const step = dotStep(zoom);
      for (const offset of [0, 7, -5, 123.4, -9876.5]) {
        const shift = dotsShift(offset, step, 1000);
        expect(shift).toBeGreaterThanOrEqual(0);
        expect(shift).toBeLessThanOrEqual(step);
        // A dot of the layer (it starts DotsMargin before the viewport, a dot at each half step) lands on the world's dots: offset + k × step.
        const dot = -DotsMargin + shift + step / 2;
        const off = dotFirst(dot - offset, step);
        expect(Math.min(off, step - off)).toBeLessThan(0.01);
      }
    }
    expect(dotStep(ZoomMax)).toBeLessThanOrEqual(DotsMargin);
    // Whole device pixels, so the layer is never drawn between two.
    expect(dotsShift(0.3, 18, 2) * 2 % 1).toBe(0);
  });
  it('fits the marching dashes to the card', () => {
    expect(outlineOf('running', true)).toBe('marching');
    expect(outlineOf('running', false)).toBe('solid');
    expect(outlineOf('waiting', true)).toBe('waiting');
    expect(outlineOf('completed', true)).toBe('none');
    expect(activityPhase(2400, true)).toBeCloseTo(0.5);
    expect(activityPhase(2400, false)).toBe(0);
    const p = perimeter(498, 238, 11);
    const dash = dashForCard(500, 240);
    expect((p / (dash[0]! + dash[1]!)) % 1).toBeCloseTo(0, 6);
    expect(dashFitting(0)).toEqual([9, 7]);
    expect(barHeight(0, 0)).toBe(8);
  });
});

describe('saved sizes (GraphBlockPreferences)', () => {
  it('sets, clears and normalizes', () => {
    const id = nodeId('p', 'request');
    const set = setBlockSize(null, id, { width: 100, height: 5000 });
    expect(set).toEqual({ [id]: { width: 300, height: 1200 } });
    expect(setBlockSize(set, id, null)).toBeNull();
    expect(splittingExecution(set)).toEqual({ [nodeId('p', 'execution')]: { width: 300, height: 1200 } });
    const record = nodeId('p', 'plan-record:x');
    expect(setBlockSize(null, record, { width: 300, height: 50 })).toEqual({ [record]: { width: 300, height: 104 } });
    expect(normalizeForRuns({ ...set, [nodeId('gone', 'request')]: { width: 400, height: 400 } }, [run('p', 'running')])).toEqual(set);
  });
});

describe('own-send camera move (natural submit B2)', () => {
  it('starts where the camera was and ends on the request, easing out', () => {
    const from = { x: 10, y: -400 }, to = { x: 10, y: -1200 };
    expect(cameraTween(from, to, 0)).toEqual(from);
    expect(cameraTween(from, to, 1)).toEqual(to);
    expect(cameraTween(from, to, 2)).toEqual(to);
    expect(cameraTween(from, to, -1)).toEqual(from);
    const half = cameraTween(from, to, 0.5);
    expect(half.x).toBe(10);
    // Past halfway by half the time: the standard easing decelerates.
    expect(half.y).toBeLessThan(-800);
    expect(half.y).toBeGreaterThan(-1200);
  });
});


import { lazy, Suspense, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react';
import { Icon } from '../icons/Icon';
import { useServices, useStoreSelector } from '../app/context';
import type { GraphBlockSize, MightyGraphRun, RunSession } from '../store/models';
import { Block, type BlockActions, type BlockExtra } from './Block';
import {
  FollowWatch, aimLanding, arrivedResult, cameraOffset, cameraTween, centredX, progressSafeLeft, ProgressFoldWidth, dotRadius, dotStep, dotsShift, dotsShown, followTargetID, resultCamera, runsChanged, scrollsBy, showsTop, trimAnchor,
  wheelRoute, wheelScrollTarget, WheelGestureGapMs, zoomAnchored, zoomIn, zoomLabel, zoomOut, ZoomDefault, ZoomMax, ZoomMin, type Point,
} from './camera';
import {
  emptyResultIds, finished, isAuxiliaryKind, isFixed, isResizable, latestResultID, makeLayout, MaximumHeight, MaximumWidth, minimumBlockSize, normalizedSizeFor,
  nodeId, resultDrag, resultDragSides, runNodeId, runOf, WorkSuffix, type Layout, type Size,
} from './layout';
import { CardHeight, DelegateKind, focusedRequest, liveFlowIds, logRowsFit, logTarget, progressRequest, requestSpans, type RequestSpan } from './flow';
import { followRunIndex, newestRequestRun } from './notice';
import { DelegateLineNode, FlowCardNode, FlowLogStrip, ProgressPanel } from './FlowParts';
import { blockExtras } from './extras';
import { settledRunsKey } from './runChangesModel';
import { edgesCursor, pinnedAnchor, pinnedCamera, pinnedUnit, resizedSize, resizeTarget, type Edges } from './resize';
import { setBlockSize } from './prefs';
import { shareRuns } from './share';
import { stepsModel } from './steps';
import { StepsPanel } from './StepsPanel';
import type { MightyBlockCopy, MightyPane, MightyFile } from './types';
import { ReferenceContext, useReferenceClick, useReferenceKey } from '../conversation/referenceClick';
import { holding, pendingRun, pendingRunId, requestState, runsFor, type PendingRequest } from '../pending/requests';
import { submitScroll } from '../pending/submitScroll';
import { Motion, reducesMotion } from '../theme/tokens';
import type { Translate } from '../i18n/locale';

const Overscan = 300;
const EMPTY_EXTRA: BlockExtra = Object.freeze({});
const LazyRunChanges = lazy(() => import('./RunChanges').then((m) => ({ default: m.RunChanges })));

/** The run of the block a click landed in, for the reference bubble's 변경사항. */
const blockRun = (target: Element) => {
  const id = target.closest<HTMLElement>('[data-block-id]')?.dataset.blockId;
  return id ? runOf(id) : null;
};

/**
 * A block the diagram opens on (MightyGraphCamera.Anchor.Reaim): selected, the camera on it. A block drawn as a card opens into its block first, as a
 * click on the card does; in the role view a block a folded card hides stands behind that card, which opens (FlowLog.drawnNode); else its request.
 */
export interface Aim { nodeId: string; fallbackNodeId?: string }

/**
 * What the diagram keeps for its pane while the timeline or the pipeline shows, as the Mac's MightyGraphView keeps its own state across its view
 * modes (openedCards, resultHeights): the cards the user opened and the newest result's measured height, so the diagram comes back as it was and a
 * reveal from another view lands on the layout the user left.
 */
export interface DiagramMemory {
  opened: ReadonlySet<string>;
  setOpened: Dispatch<SetStateAction<ReadonlySet<string>>>;
  measured: Readonly<Record<string, number>>;
  setMeasured: Dispatch<SetStateAction<Record<string, number>>>;
}

/** The point of an aimed block held in place while the diagram settles: its top-left corner, where its header starts. */
const HeldCorner = { x: 0, y: 0 };

interface Drag { id: string; edges: Edges; start: Size; origin: { x: number; y: number }; layout: Layout; target: 'result' | 'plan' | 'block' }

/** The step the edges' SVG box grows by (Diagram). */
const EdgesBoxStep = 1024;

/** Whether a scroll box from `target` up to its `block` can move by the wheel's deltas (the wheel handler, once per gesture). */
function scrollsInside(target: Element | null, block: HTMLElement | null, dx: number, dy: number): boolean {
  for (let element = target; element && block?.contains(element); element = element.parentElement) {
    if (element.scrollHeight <= element.clientHeight + 1 && element.scrollWidth <= element.clientWidth + 1) continue;
    const style = getComputedStyle(element);
    const scrolls = (overflow: string) => overflow === 'auto' || overflow === 'scroll';
    if (scrolls(style.overflowY) && scrollsBy(element.scrollTop, element.clientHeight, element.scrollHeight, dy)) return true;
    if (scrolls(style.overflowX) && scrollsBy(element.scrollLeft, element.clientWidth, element.scrollWidth, dx)) return true;
  }
  return false;
}

/** Sets an attribute only when its value changes (Diagram's applyCamera). */
function setChanged(element: Element, name: string, value: string): void {
  if (element.getAttribute(name) !== value) element.setAttribute(name, value);
}

/** The points of an edge (macOS MightyGraphLayout.route): bottom-centre out, one turn 26 from the joining end, top-centre in; a side edge out of the
 * spine block's trailing edge at its card's middle, along a lane in the gap, into the leading edge of the block it launched. */

export function edgeRoute(from: { x: number; y: number; w: number; h: number }, to: { x: number; y: number; w: number; h: number }, joins: boolean, side = false): { x: number; y: number }[] {
  if (side) {
    const start = { x: from.x + from.w, y: from.y + Math.min(from.h / 2, CardHeight / 2) };
    const end = { x: to.x, y: to.y + Math.min(to.h / 2, CardHeight / 2) };
    const lane = start.x + Math.min(22, Math.max(0, (end.x - start.x) / 2));
    return [start, { x: lane, y: start.y }, { x: lane, y: end.y }, end];
  }
  const sx = from.x + from.w / 2, sy = from.y + from.h, ex = to.x + to.w / 2, ey = to.y;
  const clearance = Math.min(26, Math.max(0, (ey - sy) / 2));
  const middle = joins ? ey - clearance : sy + clearance;
  return [{ x: sx, y: sy }, { x: sx, y: middle }, { x: ex, y: middle }, { x: ex, y: ey }];
}

/** An edge as one path: its legs and an arrow head that points the way the last leg runs (down into a block's top, or across into a side block). */
export function edgePath(points: readonly { x: number; y: number }[]): string {
  if (points.length < 2) return '';
  const end = points[points.length - 1]!;
  let from = points[points.length - 2]!;
  for (let i = points.length - 2; i >= 0 && from.x === end.x && from.y === end.y; i--) from = points[i]!;
  const length = Math.max(0.001, Math.hypot(end.x - from.x, end.y - from.y));
  const dx = (end.x - from.x) / length, dy = (end.y - from.y) / length;
  const legs = points.map((p, i) => (i === 0 ? 'M' : 'L') + p.x + ' ' + p.y).join('');
  const wing = (sign: number) => `${end.x - 6 * dx - sign * 4 * dy} ${end.y - 6 * dy + sign * 4 * dx}`;
  return `${legs}M${wing(1)}L${end.x} ${end.y}L${wing(-1)}`;
}

/** The edges as one path each (MightyGraph.cs edges). */
function edgePaths(layout: Layout, running: ReadonlySet<string>): { quiet: string; live: string } {
  const frames = new Map(layout.nodes.map((n) => [n.id, n.frame]));
  let quiet = '', live = '';
  for (const edge of layout.edges) {
    const from = frames.get(edge.source), to = frames.get(edge.target);
    if (!from || !to) continue;
    const d = edgePath(edgeRoute(from, to, edge.joins, edge.side === true));
    if (running.has(edge.target)) live += d; else quiet += d;
  }
  return { quiet, live };
}

/**
 * The copy of a sent request's blocks until the host's come (natural submit B1): "다음 요청" with the pane's agent and no pill
 * (its 전송 중 is the block's pending mark), 실행 waiting for its answer, and for a request that did not start, 실행 stopped and
 * a failed result carrying the host's message.
 */
export function pendingCopies(request: PendingRequest, id: string, standIn: boolean, provider: string, t: Translate): Map<string, MightyBlockCopy> {
  const failed = request.phase === 'failed';
  const copy = (kind: string, title: string, status: string, tone: string): MightyBlockCopy =>
    ({ kind, title, state: '', status, tone, help: '', indicator: 'none', outline: 'none', fromRecord: false, provider: kind === 'request' ? provider : undefined });
  const copies = new Map([[nodeId(id, 'request'), copy('request', t('graph.block.nextRequest'), '', 'run')]]);
  // On the host's own run (it has arrived, its copy is on its way) only the request block is covered: its 실행 and result
  // wait for their own copy, so a run that finished at once never shows 요청 실패 meanwhile.
  if (standIn) copies.set(nodeId(id, WorkSuffix), copy('work', t('graph.block.execution'), failed ? 'stopped' : 'starting', failed ? 'stop' : 'idle'));
  if (failed) copies.set(nodeId(id, 'result'), copy('result', t('graph.block.resultError'), 'error', 'err'));
  return copies;
}

/**
 * The Mighty diagram (UI/MightyGraph.cs): the page's own layout of the pane's runs, its blocks as positioned cards in one
 * transformed layer, and one SVG under them for the dot grid and the edges. Pan, zoom and the follow camera move that layer
 * and the SVG's transforms directly, so moving the view draws no block; blocks draw only when their props change, and a
 * block off screen draws its header alone.
 */
export function Diagram({ session, mighty, memory, aim, onAimed, pending: request, own = 0 }: {
  session: RunSession; mighty: MightyPane; memory: DiagramMemory; aim?: Aim | null; onAimed?: () => void;
  /** The request the composer sent that the host's run has not replaced yet: drawn as the newest run. */
  pending?: PendingRequest;
  /** Counts the pane's own sends: each moves the camera to the new request over motion.scroll. */
  own?: number;
}) {
  const { bridge, t } = useServices();
  const paneId = session.id;
  const planText = useStoreSelector(
    (state) => ((state.cards[paneId] ?? []).find((c) => c.kind === 'plan') as { plan?: string } | undefined)?.plan,
  );
  const inputs = mighty.inputs;
  const viewportRef = useRef<HTMLDivElement>(null);
  /** The flow's whole box: the diagram's view and the session log strip under it (logRowsFit). */
  const rootRef = useRef<HTMLDivElement>(null);
  const [room, setRoom] = useState(0);
  const worldRef = useRef<HTMLDivElement>(null);
  const dotsLayer = useRef<SVGSVGElement>(null);
  const dotsRef = useRef<SVGPatternElement>(null);
  const dotRef = useRef<SVGCircleElement>(null);
  const dotsRect = useRef<SVGRectElement>(null);
  const camera = useRef<Point>({ x: 0, y: 0 });
  /** The camera's move to an own send's request: from where it was to where the follow puts it, over motion.scroll. */
  const tween = useRef<{ from: Point; to: Point; start: number; frame: number } | null>(null);
  const ownSeen = useRef(own);
  const watch = useRef(new FollowWatch());
  const runIds = useRef<string[] | null>(null);
  const shared = useRef<readonly MightyGraphRun[]>([]);
  const [viewport, setViewport] = useState<Size>({ w: 0, h: 0 });
  const [zoom, setZoom] = useState(ZoomDefault);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  /** The cards the user opened into their full blocks (macOS MightyGraphView openedCards) and the newest result's measured height, kept by the
   * Mighty view while another view shows. */
  const { opened, setOpened, measured, setMeasured } = memory;
  const [selected, setSelected] = useState<string | null>(null);
  /** 요청 변경 검토: the runs whose request's changes it can show, and the run it is open for. */
  const [changedRuns, setChangedRuns] = useState<ReadonlySet<string>>(() => new Set());
  const [changesRun, setChangesRun] = useState<string | null>(null);
  const [visibleKey, setVisibleKey] = useState('');
  const [live, setLive] = useState<{ id: string; size: GraphBlockSize; target: Drag['target'] } | null>(null);
  const [pending, setPending] = useState<{ sizes: Record<string, GraphBlockSize> | null; base: unknown } | null>(null);
  const drag = useRef<Drag | null>(null);
  /** The corner across from a drag's sides, kept at its place on screen while and after the drag until the view is moved. */
  const pin = useRef<{ id: string; unit: { x: number; y: number }; anchor: Point } | null>(null);
  /**
   * The block an aim landed on, kept where it landed while the diagram settles under it until the view is moved: the newest result is measured
   * only once it is drawn, the session log strip comes with the host's next push, a copy may come later, and each moves what stands below.
   */
  const held = useRef<{ id: string; anchor: Point } | null>(null);
  // A file named in a block opens in the pane's reference bubble; kept in a ref so the blocks' actions keep their identity.
  const referenceShown = useContext(ReferenceContext);
  const showReference = useRef(referenceShown);
  showReference.current = referenceShown;
  const onReferenceClick = useReferenceClick(blockRun);
  const onReferenceKey = useReferenceKey(blockRun);
  const loadingOlder = useRef<unknown>(null);

  const hostRuns = useMemo(() => {
    const next = shareRuns(shared.current, [...mighty.older, ...(session.graphRuns ?? [])]);
    shared.current = next;
    return next;
  }, [mighty.older, session.graphRuns]);
  // A sent request stands in as the newest run until the host's comes (natural submit B1); once the host's run is there, its
  // blocks wear the request's copy and 전송 중 until the host pushes their own copy (that comes up to ~120 ms after the run).
  const arrivedKey = request && request.phase !== 'failed' ? runsFor(request, session).join('\n') : '';
  const standIn = request !== undefined && arrivedKey === '';
  /** The run whose blocks carry the request's copy and pending mark: the stand-in, else the host's newest run for it. */
  const pendingRunIdOf = !request ? null : standIn ? pendingRunId(request) : arrivedKey.split('\n').pop() ?? null;
  const runs = useMemo(() => (request && standIn ? [...hostRuns, pendingRun(request, session.provider)] : hostRuns), [hostRuns, request, standIn, session.provider]);
  const copies = useMemo(() => (request && pendingRunIdOf ? pendingCopies(request, pendingRunIdOf, standIn, session.provider, t) : null), [request, pendingRunIdOf, standIn, session.provider, t]);
  const pendingState = useMemo(() => (request ? requestState(request) : null), [request]);

  // A released drag shows its size at once; the saved sizes coming back from the host take over.
  const savedSizes = pending && pending.base === session.graphBlockSizes ? pending.sizes : session.graphBlockSizes ?? null;
  const latest = latestResultID(runs);
  // The role view of a Claude pane: cards, delegate lines, steps and the plan column (the host says when).
  const fanOut = inputs?.fanOut === true;
  // Every agent pane the host sends progress for has the progress panel (macOS MightyGraphView showsProgress); the role view is Claude's alone.
  const showsProgress = inputs?.progress !== undefined && inputs?.progress !== null;
  const blockSizes = useMemo(() => (live && live.target === 'block' ? { ...(savedSizes ?? {}), [live.id]: live.size } : savedSizes), [savedSizes, live]);
  // A settled result with nothing to show is drawn as its header alone (layout emptyResultIds).
  // Kept by its ids, so a push that changes no block's emptiness lays nothing out again.
  const emptyKey = [...emptyResultIds(runs, (id) => mighty.blocks[id], inputs?.resultFilesRunID ?? null)].join('\n');
  const emptyResults = useMemo(() => new Set(emptyKey === '' ? [] : emptyKey.split('\n')), [emptyKey]);
  const layout = useMemo(() => makeLayout(runs, session.draft, session.status === 'running', expanded, {
    resultFilesRunID: inputs?.resultFilesRunID ?? null,
    viewport: viewport.w > 0 && viewport.h > 0 ? viewport : null,
    zoom,
    sharedResultSize: live?.target === 'result' ? live.size : session.graphResultSize ?? null,
    retainedStart: inputs?.retainedStart ?? 0,
    history: inputs?.history !== undefined,
    resultContentHeight: latest && !live ? measured[latest] ?? null : null,
    blockSizes,
    planRunID: inputs?.planRunID ?? null,
    planRecords: inputs?.planRecords ?? [],
    planSize: live?.target === 'plan' ? live.size : session.graphPlanSize ?? null,
    splitsExecution: true,
    galleries: inputs?.galleries ?? [],
    executions: inputs?.executions ?? [],
    fanOut: fanOut ? { opened } : null,
    emptyResults,
  }), [runs, session.draft, session.status, expanded, inputs, viewport, zoom, live, session.graphResultSize, session.graphPlanSize, latest, measured, blockSizes, fanOut, opened, emptyResults]);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;
  // Whether the progress panel is drawn this pass: the camera keeps its room on the left (set where the panel is worked out, below).
  const panelShown = useRef(false);
    const emptyRef = useRef(emptyResults);
  emptyRef.current = emptyResults;
  // The progress panel's request (macOS MightyGraphView focusedRunID, pinnedRunID): the one in view, read as the camera moves; a new request it is
  // pinned to until the user scrolls themselves (a wheel, a drag), whatever else is in view.
  const [focusedRun, setFocusedRun] = useState<string | null>(null);
  const [pinnedRun, setPinnedRun] = useState<string | null>(null);
  const pinnedRef = useRef(pinnedRun);
  /** An own send waiting for its request to be laid out, by the newest request it was sent after ('' for none). */
  const sendTop = useRef<string | null>(null);
  /** The newest work's result block seen in the pass before (undefined before the first pass): one that comes in is shown. */
  const resultSeen = useRef<string | null | undefined>(undefined);
  pinnedRef.current = pinnedRun;
  const spans = useMemo<RequestSpan[]>(() => {
    if (!showsProgress) return [];
    // The longest prefix first: a notice run's id begins with its request's.
    const prefixes = runs.map((run, index) => ({ prefix: runNodeId(run, ''), index })).sort((a, b) => b.prefix.length - a.prefix.length);
    return requestSpans(layout.nodes.filter((n) => n.kind !== 'history' && n.kind !== 'draft'), runs,
      (id) => prefixes.find((p) => id.startsWith(p.prefix))?.index ?? null);
  }, [showsProgress, layout, runs]);
  const spansRef = useRef(spans);
  spansRef.current = spans;
  const newestRequestID = newestRequestRun(runs)?.id ?? null;
  const seenNewest = useRef(newestRequestID);
  useEffect(() => {
    if (newestRequestID !== null && newestRequestID !== seenNewest.current && showsProgress) setPinnedRun(newestRequestID);
    seenNewest.current = newestRequestID;
  }, [newestRequestID, showsProgress]);

  const running = useMemo(() => new Set(Object.entries(mighty.blocks).filter(([, b]) => b.tone === 'run').map(([id]) => id)), [mighty.blocks]);
  // The role view's own nodes in motion: a delegate line that leads to a running block (flow.ts liveFlowIds).
  const liveIds = useMemo(() => (fanOut ? liveFlowIds(layout, running) : running), [fanOut, layout, running]);
  const edges = useMemo(() => edgePaths(layout, liveIds), [layout, liveIds]);
  // The edges' box: the layout's extent from its origin, in steps of EdgesBoxStep so a block growing a line at a time does not
  // resize the SVG (a resize paints all of it again).
  const edgesX = Math.floor(layout.originX / EdgesBoxStep) * EdgesBoxStep, edgesY = Math.floor(layout.originY / EdgesBoxStep) * EdgesBoxStep;
  const edgesW = Math.ceil((layout.originX + layout.size.w + 1 - edgesX) / EdgesBoxStep) * EdgesBoxStep || EdgesBoxStep;
  const edgesH = Math.ceil((layout.originY + layout.size.h + 1 - edgesY) / EdgesBoxStep) * EdgesBoxStep || EdgesBoxStep;
  const edgesBox = useMemo(() => ({ x: edgesX, y: edgesY, w: edgesW, h: edgesH }), [edgesX, edgesY, edgesW, edgesH]);

  // ── the camera: transforms set directly, the visible set recomputed only when it changes ──
  const frame = useRef(0);
  const updateVisible = useCallback(() => {
    frame.current = 0;
    const cam = camera.current, l = layoutRef.current, vp = viewportRef.current;
    if (!vp) return;
    const w = vp.clientWidth, h = vp.clientHeight;
    const z = zoomRef.current;
    const left = (-cam.x - Overscan) / z, top = (-cam.y - Overscan) / z, right = (w - cam.x + Overscan) / z, bottom = (h - cam.y + Overscan) / z;
    const ids = l.nodes.filter((n) => n.frame.x < right && n.frame.x + n.frame.w > left && n.frame.y < bottom && n.frame.y + n.frame.h > top).map((n) => n.id).join('\n');
    setVisibleKey((key) => (key === ids ? key : ids));
    // The request in view; only a change of it is told.
    if (spansRef.current.length > 0) {
      const focused = focusedRequest(spansRef.current, -cam.y / z, h / z);
      setFocusedRun((was) => (was === focused ? was : focused));
    }
  }, []);
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  // The camera moves layers, it does not redraw them: the world (its blocks and their edges) and the dots each sit on a layer
  // of their own and only their CSS transform changes, which WebKit applies without painting. The dots' pattern is redrawn
  // only when the zoom changes its step; each value is written only when it differs, since WebKit repaints a pattern's whole
  // area on any write to it and this runs on every layout pass while a request grows its block.
  const applyCamera = useCallback(() => {
    const cam = camera.current, z = zoomRef.current;
    const world = worldRef.current, transform = `translate(${cam.x}px, ${cam.y}px) scale(${z})`;
    if (world && world.style.transform !== transform) world.style.transform = transform;
    const step = dotStep(z);
    const layer = dotsLayer.current, dots = dotsRef.current, dot = dotRef.current;
    if (layer && dots && dot) {
      setChanged(dots, 'width', String(step)); setChanged(dots, 'height', String(step));
      setChanged(dot, 'cx', String(step / 2)); setChanged(dot, 'cy', String(step / 2)); setChanged(dot, 'r', String(dotRadius(z)));
      if (dotsRect.current) setChanged(dotsRect.current, 'visibility', dotsShown(z) ? 'visible' : 'hidden');
      const shift = `translate(${dotsShift(cam.x, step)}px, ${dotsShift(cam.y, step)}px)`;
      if (layer.style.transform !== shift) layer.style.transform = shift;
    }
    if (!frame.current) frame.current = requestAnimationFrame(updateVisible);
  }, [updateVisible]);
  useEffect(() => () => { if (frame.current) cancelAnimationFrame(frame.current); }, []);

  const stopTween = useCallback(() => {
    if (tween.current) cancelAnimationFrame(tween.current.frame);
    tween.current = null;
  }, []);
  /** Moves the camera from `from` to `to` along the standard easing; each frame sets it and moves the layer. */
  const startTween = useCallback((from: Point, to: Point) => {
    stopTween();
    const move = { from, to, start: performance.now(), frame: 0 };
    tween.current = move;
    const step = () => {
      if (tween.current !== move) return;
      const progress = (performance.now() - move.start) / Motion.scroll;
      camera.current = cameraTween(move.from, move.to, progress);
      applyCamera();
      if (progress < 1) move.frame = requestAnimationFrame(step);
      else tween.current = null;
    };
    camera.current = from;
    move.frame = requestAnimationFrame(step);
  }, [applyCamera, stopTween]);
  useEffect(() => stopTween, [stopTween]);

  useLayoutEffect(() => {
    const element = viewportRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setViewport((v) => (v.w === element.clientWidth && v.h === element.clientHeight ? v : { w: element.clientWidth, h: element.clientHeight })));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  useLayoutEffect(() => {
    const element = rootRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setRoom((h) => (h === element.clientHeight ? h : element.clientHeight)));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const frameOf = (id: string | null) => (id === null ? null : layoutRef.current.nodes.find((n) => n.id === id)?.frame ?? null);
  /** The left inset the camera keeps while the progress panel stands over the diagram (camera progressSafeLeft). */
  const safeLeftOf = (vp: Size, z: number) => progressSafeLeft(panelShown.current, vp, z);

  // ── follow and the run-id rules after every layout pass (MightyGraphFollow, AimGraphCamera) ──
  useLayoutEffect(() => {
    if (viewport.w <= 0 || viewport.h <= 0) return;
    // Where the camera stands before this pass: an own send moves from here (a move under way, from where it is now).
    const before = { ...camera.current };
    const ownSend = own !== ownSeen.current;
    ownSeen.current = own;
    if (ownSend && submitScroll('diagram', true, true, watch.current.following).scroll === 'toRequest') watch.current.start();
    // The diagram with its progress panel (macOS handleOwnSubmit): an own send's request goes to the top once it is there, the
    // request in view and the one the panel is pinned to, with its work coming in under it.
    if (ownSend && showsProgress) sendTop.current = newestRequestRun(runs)?.id ?? '';
    // The user's requests: a notice run (notice.ts) never starts following by itself; a following camera moves on to it.
    const ids = runs.filter((r) => r.noticeRun !== true).map((r) => r.id);
    const previous = runIds.current;
    const change = runsChanged(previous, ids, watch.current.following);
    runIds.current = ids;
    // A send, and a request that came or went, move the camera by their own rules: an aimed block is held no longer.
    if (ownSend || change !== 'none') held.current = null;
    if (change === 'first' || change === 'follow') {
      watch.current.start();
      camera.current = { ...camera.current, x: centredX(viewport, zoom, safeLeftOf(viewport, zoom)) };
    } else if (change === 'trim') {
      const anchor = trimAnchor(previous ?? [], ids, selected, new Set(layout.nodes.map((n) => n.id)));
      const target = anchor.kind === 'reaim' ? frameOf(anchor.nodeID) : null;
      if (anchor.kind === 'reaim' && target) camera.current = cameraOffset(target, viewport, zoom, anchor.alignTop, false, safeLeftOf(viewport, zoom));
    }
    const targetID = followTargetID(layout, runs);
    // The user's own swipe or drag under way: the camera is theirs until it ends.
    const swipe = wheelGesture.current;
    const userMoving = panning.current !== null || (swipe !== null && !swipe.inner && performance.now() - swipe.at < WheelGestureGapMs);
    const next = watch.current.observe(targetID, frameOf(targetID), camera.current, viewport, zoom, userMoving);
    if (next) camera.current = next;
    if (sendTop.current !== null) {
      const newest = newestRequestRun(runs);
      const frame = newest && newest.id !== sendTop.current ? frameOf(runNodeId(newest, 'request')) : null;
      if (newest && frame) {
        sendTop.current = null;
        watch.current.stop();
        camera.current = cameraOffset(frame, viewport, zoom, true, false, safeLeftOf(viewport, zoom));
        setPinnedRun(newest.id);
      }
    }
    // The newest work's result came in: the camera glides to it, its top kept in view, following on from there when it all fits (a
    // send that pinned its request to the top stopped the follow, so the result would land out of sight under the work).
    const followRun = runs[followRunIndex(runs) ?? -1];
    const resultID = followRun ? runNodeId(followRun, 'result') : null;
    const resultFrame = frameOf(resultID);
    const shownResult = resultFrame ? resultID : null;
    const revealed = !aim && !userMoving && arrivedResult(resultSeen.current, shownResult);
    // The last result seen stays until another comes in, so one that leaves the layout for a pass and returns is not shown again.
    if (resultSeen.current === undefined || shownResult !== null) resultSeen.current = shownResult;
    if (revealed && resultFrame && resultID !== null) {
      sendTop.current = null;
      held.current = null;
      camera.current = resultCamera(resultFrame, viewport, zoom, safeLeftOf(viewport, zoom));
      watch.current.moved(targetID, frameOf(targetID), camera.current, viewport, zoom);
      if (!watch.current.following) held.current = { id: resultID, anchor: pinnedAnchor(HeldCorner, resultFrame, camera.current, zoom) };
    }
    // A block asked for from another view: after the run-id rules, so a send is followed first and this wins.
    if (aim) {
      const landing = aimLanding(aim, layout, runs, fanOut, opened);
      if (landing && 'open' in landing) {
        // A card opens into its block, as a click on it does; the aim lands on that block in the next pass.
        setOpened((s) => new Set(s).add(landing.open));
      } else if (landing) {
        const target = landing.land;
        pin.current = null;
        camera.current = cameraOffset(target.frame, viewport, zoom, false, false, safeLeftOf(viewport, zoom));
        watch.current.moved(targetID, frameOf(targetID), camera.current, viewport, zoom);
        setSelected(target.id);
        // Not following from there: the block stays where it landed while what is drawn above it settles, until the view is moved.
        held.current = watch.current.following ? null : { id: target.id, anchor: pinnedAnchor(HeldCorner, target.frame, camera.current, zoom) };
        onAimed?.();
      }
    } else if (held.current) {
      const frame = frameOf(held.current.id);
      if (frame) camera.current = pinnedCamera(held.current.anchor, HeldCorner, frame, zoom);
      else held.current = null;
    }
    // A resized block: the layout recentres it, the camera keeps the corner across from the dragged sides where it was.
    const pinned = pin.current;
    const pinnedFrame = pinned ? frameOf(pinned.id) : null;
    if (pinned && pinnedFrame) camera.current = pinnedCamera(pinned.anchor, pinned.unit, pinnedFrame, zoom);
    else pin.current = null;
    // An own send glides to its request (motion.scroll); Reduce Motion moves at once. A move under way keeps going to where
    // this pass put the camera.
    const moving = tween.current;
    if ((ownSend || revealed) && !aim && !reducesMotion()) startTween(before, camera.current);
    else if (moving && !aim && !ownSend) {
      // This pass moved the camera (the request grew, the follow went on): the move heads there instead.
      if (Math.abs(camera.current.x - before.x) > 0.5 || Math.abs(camera.current.y - before.y) > 0.5) moving.to = camera.current;
      camera.current = cameraTween(moving.from, moving.to, (performance.now() - moving.start) / Motion.scroll);
    }
    else if (moving) stopTween();
    applyCamera();
  }, [layout, viewport, zoom, aim, own]);

  /**
   * The user moved the view: it follows on while the block it followed still ends in view (FollowWatch.moved). That block, not
   * one that came under it during the move, is what the move is judged against, so a new block below never ends the following;
   * the pass after the move brings it into view (`settleFollow`).
   */
  const userMoved = useCallback(() => {
    pin.current = null;
    held.current = null;
    const l = layoutRef.current;
    const targetID = followTargetID(l, shared.current);
    const followed = watch.current.followedID;
    const judged = followed !== null && frameOf(followed) ? followed : targetID;
    watch.current.moved(judged, frameOf(judged), camera.current, { w: viewportRef.current?.clientWidth ?? 0, h: viewportRef.current?.clientHeight ?? 0 }, zoomRef.current);
    // Panning up to the history block reads older requests (LoadOlderAtTop).
    const history = l.nodes.find((n) => n.kind === 'history');
    if (history && inputsRef.current?.history?.acts && loadingOlder.current !== inputsRef.current && showsTop(camera.current.y, zoomRef.current, history.frame.y)) {
      loadingOlder.current = inputsRef.current;
      void bridge.call('mightyLoadOlder', { paneId });
    }
  }, [bridge, paneId]);
  const inputsRef = useRef(inputs);
  inputsRef.current = inputs;

  /** The camera to a block, as another view's aim does: it stops following; a block (not a card, a line or the band) is selected. */
  const reveal = useCallback((id: string) => {
    const l = layoutRef.current;
    const node = l.nodes.find((n) => n.id === id);
    const vp = { w: viewportRef.current?.clientWidth ?? 0, h: viewportRef.current?.clientHeight ?? 0 };
    if (!node || vp.w <= 0 || vp.h <= 0) return;
    stopTween();
    pin.current = null;
    held.current = null;
    camera.current = cameraOffset(node.frame, vp, zoomRef.current, false, false, safeLeftOf(vp, zoomRef.current));
    const targetID = followTargetID(l, shared.current);
    watch.current.moved(targetID, frameOf(targetID), camera.current, vp, zoomRef.current);
    if (!isFixed(node)) setSelected(id);
    applyCamera();
  }, [applyCamera, stopTween]);
  /** A card opened into its block in place, or folded back (macOS openedCards). */
  const openCard = useCallback((id: string, open: boolean) => {
    watch.current.userChangedLayout();
    setOpened((s) => { if (s.has(id) === open) return s; const n = new Set(s); if (open) n.add(id); else n.delete(id); return n; });
  }, [setOpened]);
  const openFlowCard = useCallback((id: string) => openCard(id, true), [openCard]);
  /** A card's click: a finished background block's copy in its notice run, shown and selected; any other card opens in place. */
  const activateCard = useCallback((id: string, copy: string | undefined) => {
    if (copy && layoutRef.current.nodes.some((n) => n.id === copy)) reveal(copy);
    else openCard(id, true);
  }, [reveal, openCard]);
  /** A log row's or a plan row's click: the block, or what stands for it on the diagram while a folded card hides it. */
  const revealFlow = useCallback((nodeID: string) => {
    const drawn = new Set(layoutRef.current.nodes.map((n) => n.id));
    const target = logTarget(nodeID, shared.current, (run, suffix) => runNodeId(run, suffix), (id) => drawn.has(id));
    if (target) reveal(target);
  }, [reveal]);
  const toggleLog = useCallback(() => {
    void bridge.call('mightyLogCollapsed', { paneId, collapsed: inputsRef.current?.log?.collapsed !== true });
  }, [bridge, paneId]);

  const pan = useCallback((dx: number, dy: number) => {
    stopTween();
    held.current = null;
    // The user's own scroll: the progress panel reads the request in view again, and a send's move to the top is off.
    if (pinnedRef.current !== null) setPinnedRun(null);
    sendTop.current = null;
    camera.current = { x: camera.current.x + dx, y: camera.current.y + dy };
    applyCamera();
  }, [applyCamera, stopTween]);

  /** Once the user's move is over, a following camera catches up with what came meanwhile (a block under the newest, its growth). */
  const settleTimer = useRef(0);
  const settleFollow = useCallback((after: number) => {
    window.clearTimeout(settleTimer.current);
    settleTimer.current = window.setTimeout(() => {
      const vp = viewportRef.current;
      if (!vp || panning.current) return;
      const targetID = followTargetID(layoutRef.current, shared.current);
      const next = watch.current.observe(targetID, frameOf(targetID), camera.current, { w: vp.clientWidth, h: vp.clientHeight }, zoomRef.current);
      if (next) { camera.current = next; applyCamera(); }
    }, after);
  }, [applyCamera]);
  useEffect(() => () => window.clearTimeout(settleTimer.current), []);

  // ── wheel: a selected or attached block under the pointer scrolls its body, anything else pans ──
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const wheelGesture = useRef<{ at: number; inner: boolean } | null>(null);
  useEffect(() => {
    const element = viewportRef.current;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      // The steps panel scrolls its own list.
      if ((event.target as Element).closest?.('.mighty-steps, .mighty-progress')) return;
      const hovered = (event.target as Element).closest?.('[data-block-id]') as HTMLElement | null;
      const id = hovered?.dataset.blockId ?? null;
      const kind = hovered?.dataset.kind ?? '';
      const dx = event.deltaX !== 0 ? event.deltaX : event.shiftKey ? event.deltaY : 0;
      const dy = event.shiftKey && event.deltaX === 0 ? 0 : event.deltaY;
      // A gesture keeps its route; a new one goes into the block only where something in it scrolls that way.
      const route = wheelRoute(wheelGesture.current, event.timeStamp, wheelScrollTarget(selectedRef.current, id, isAuxiliaryKind(kind)) !== null,
        () => scrollsInside(event.target as Element, hovered, dx, dy));
      wheelGesture.current = route;
      if (route.inner) return;
      event.preventDefault();
      pan(-dx, -dy);
      userMoved();
      settleFollow(WheelGestureGapMs);
    };
    element.addEventListener('wheel', wheel, { passive: false });
    return () => element.removeEventListener('wheel', wheel);
  }, [pan, userMoved, settleFollow]);

  // ── background drag pans and clears the selection; a press on a block selects it ──
  const panning = useRef<{ x: number; y: number; pointer: number } | null>(null);
  /** The block and sides a press at the client point would resize: the corner handle, then a band along each border. */
  const resizeAt = (clientX: number, clientY: number) => {
    const vp = viewportRef.current;
    if (!vp) return null;
    const box = vp.getBoundingClientRect(), cam = camera.current, z = zoomRef.current;
    const frames = layoutRef.current.nodes.filter((n) => isResizable(n.kind) && !isFixed(n) && !emptyRef.current.has(n.id))
      .map((n) => ({ id: n.id, frame: { x: n.frame.x * z + cam.x, y: n.frame.y * z + cam.y, w: n.frame.w * z, h: n.frame.h * z } }));
    return resizeTarget(clientX - box.left, clientY - box.top, frames, z);
  };
  const resizeCursor = useRef('');
  const showResizeCursor = (cursor: string) => {
    const vp = viewportRef.current;
    if (!vp || resizeCursor.current === cursor) return;
    resizeCursor.current = cursor;
    vp.style.cursor = cursor;
    if (cursor) vp.dataset.resizeCursor = ''; else delete vp.dataset.resizeCursor;
  };
  const onPointerDown = (event: React.PointerEvent) => {
    if (event.button !== 0) return;
    // The user acts in the view: an aimed block is held no longer.
    held.current = null;
    const hit = resizeAt(event.clientX, event.clientY);
    if (hit && !(event.target as Element).closest('button')) { actions.startResize(hit.id, hit.edges, event); return; }
    const block = (event.target as Element).closest('[data-block-id]') as HTMLElement | null;
    if (block) {
      // A card, a delegate line and the plan column are never selected (Core MightyGraphLayout.IsFixed).
      if (block.dataset.kind !== 'history' && block.dataset.fixed !== '1') setSelected(block.dataset.blockId ?? null);
      return;
    }
    setSelected(null);
    panning.current = { x: event.clientX, y: event.clientY, pointer: event.pointerId };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    viewportRef.current?.focus();
  };
  const onPointerMove = (event: React.PointerEvent) => {
    const d = drag.current;
    if (d) {
      const least = minimumBlockSize(d.layout.nodes.find((n) => n.id === d.id)?.kind ?? '');
      const dragged = resizedSize(d.edges, d.start, event.clientX - d.origin.x, event.clientY - d.origin.y, zoomRef.current, least, { w: MaximumWidth, h: MaximumHeight });
      const size = { width: dragged.w, height: dragged.h };
      if (d.target === 'block') setLive({ id: d.id, size: normalizedSizeFor(size, d.id) ?? size, target: 'block' });
      else {
        const limit = d.target === 'result' ? d.layout.resultLimit : d.layout.planLimit;
        const shown = resultDrag({ w: size.width, h: size.height }, true, true, 'live', null, limit, null).live;
        if (shown) setLive({ id: d.id, size: shown, target: d.target });
      }
      return;
    }
    const p = panning.current;
    // Over the steps panel no block border is under the pointer: a press there never resizes.
    if (!p) { showResizeCursor((event.target as Element).closest('.mighty-steps, .mighty-progress') ? '' : edgesCursor(resizeAt(event.clientX, event.clientY)?.edges ?? 0)); return; }
    pan(event.clientX - p.x, event.clientY - p.y);
    panning.current = { ...p, x: event.clientX, y: event.clientY };
  };
  const endDrag = (event: React.PointerEvent | null, cancelled: boolean) => {
    const d = drag.current;
    if (d) {
      drag.current = null;
      const shown = live;
      setLive(null);
      if (cancelled || !shown || !event) return;
      const dragged = { w: shown.size.width, h: shown.size.height };
      const sides = resultDragSides(d.start, dragged);
      if (!sides.horizontal && !sides.vertical) return;
      watch.current.userChangedLayout();
      if (d.target === 'block') {
        setPending({ sizes: setBlockSize(session.graphBlockSizes, d.id, shown.size), base: session.graphBlockSizes });
        void bridge.call('mightySaveSize', { paneId, nodeId: d.id, target: 'block', width: shown.size.width, height: shown.size.height });
        return;
      }
      const saved = d.target === 'result' ? session.graphResultSize ?? null : session.graphPlanSize ?? null;
      const limit = d.target === 'result' ? d.layout.resultLimit : d.layout.planLimit;
      const fit = d.target === 'result' ? d.layout.resultWindowFit : d.layout.planWindowFit;
      const save = resultDrag(dragged, sides.horizontal, sides.vertical, 'finished', saved, limit, fit).save;
      if (save) void bridge.call('mightySaveSize', { paneId, nodeId: d.id, target: d.target, width: save.width, height: save.height });
      return;
    }
    if (panning.current) { panning.current = null; userMoved(); settleFollow(0); }
  };

  const actions = useMemo<BlockActions>(() => ({
    toggleExpand(id, open) {
      watch.current.userChangedLayout();
      setExpanded((s) => { const n = new Set(s); if (open) n.add(id); else n.delete(id); return n; });
      void bridge.call('mightyExpand', { paneId, nodeId: id, expanded: open });
    },
    startResize(id, edges, event) {
      if (event.button !== 0) return;
      event.stopPropagation();
      const l = layoutRef.current;
      const node = l.nodes.find((n) => n.id === id);
      if (!node) return;
      const target = id === latestResultID(shared.current) ? 'result' : node.kind === 'plan' ? 'plan' : 'block';
      drag.current = { id, edges, start: { w: node.frame.w, h: node.frame.h }, origin: { x: event.clientX, y: event.clientY }, layout: l, target };
      const unit = pinnedUnit(edges);
      held.current = null;
      pin.current = { id, unit, anchor: pinnedAnchor(unit, node.frame, camera.current, zoomRef.current) };
      watch.current.userChangedLayout();
      setSelected(id);
      viewportRef.current?.setPointerCapture(event.pointerId);
      viewportRef.current?.focus();
    },
    resetSize(id) {
      watch.current.userChangedLayout();
      const node = layoutRef.current.nodes.find((n) => n.id === id);
      const target = id === latestResultID(shared.current) ? 'result' : node?.kind === 'plan' ? 'plan' : 'block';
      setExpanded((s) => { if (!s.has(id)) return s; const n = new Set(s); n.delete(id); return n; });
      void bridge.call('mightySaveSize', { paneId, nodeId: id, target });
    },
    toggleFiles(runId) { void bridge.call('mightyResultFiles', { paneId, runId }); },
    fitResult() { void bridge.call('mightySaveSize', { paneId, nodeId: latestResultID(shared.current) ?? '', target: 'result' }); },
    openFile(file: MightyFile, runId: string | null) { showReference.current?.({ path: file.path, line: file.line ?? null, ...(runId ? { runId } : {}) }); },
    openInEditor(file: MightyFile) {
      void bridge.call('openInEditor', { source: 'reference', paneId, path: file.path, ...(file.line ? { line: file.line } : {}) }).catch(() => {});
    },
    loadOlder() { void bridge.call('mightyLoadOlder', { paneId }); },
    measured(id, height) { setMeasured((m) => (Math.abs((m[id] ?? -1) - height) < 1 ? m : { ...m, [id]: height })); },
    foldToCard(id) { openCard(id, false); },
    openChanges(runId) { setChangesRun(runId); },
    forkHere(runId) { void bridge.call('paneFork', { paneId, requestId: runId }).catch(() => {}); },
  }), [bridge, paneId, openCard, setMeasured]);

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key !== 'Escape') return;
    if (drag.current) { drag.current = null; setLive(null); } else setSelected(null);
  };

  const setZoomTo = (next: number) => {
    const vp = { w: viewportRef.current?.clientWidth ?? 0, h: viewportRef.current?.clientHeight ?? 0 };
    pin.current = null;
    held.current = null;
    stopTween();
    camera.current = zoomAnchored(camera.current, vp, zoom, next);
    watch.current.stop();
    setZoom(next);
  };

  // The settled runs' results offer 요청 변경 검토 once the host says their request's edits were kept (files.rs runChangesRuns).
  const settledKey = useMemo(() => settledRunsKey(runs), [runs]);
  useEffect(() => {
    let live = true;
    const ids = settledKey === '' ? [] : settledKey.split('\n');
    const take = (found: readonly string[]) => setChangedRuns((now) => (now.size === found.length && found.every((id) => now.has(id)) ? now : new Set(found)));
    if (ids.length === 0) take([]);
    else bridge.call<string[] | null>('runChangesRuns', { paneId, runIds: ids }).then((found) => { if (live) take(found ?? []); }, () => {});
    return () => { live = false; };
  }, [bridge, paneId, settledKey]);

  // ── each block's own parts, kept while they do not change ──
  const extrasCache = useRef<ReadonlyMap<string, BlockExtra>>(new Map());
  const extras = useMemo(() => {
    const next = blockExtras({ layout, runs, inputs, draft: session.draft, planHistory: session.planHistory, resultSize: session.graphResultSize, planText, latest, changedRuns, emptyResults }, extrasCache.current);
    extrasCache.current = next;
    return next;
  }, [layout, runs, inputs, session.draft, session.planHistory, session.graphResultSize, planText, latest, changedRuns, emptyResults]);

  // The sent request's block carries its pending mark (전송 중, or its failure) in place of a pill.
  const drawnExtras = useMemo(() => {
    if (!request || !pendingState || !pendingRunIdOf) return extras;
    const id = nodeId(pendingRunIdOf, 'request');
    const extra = extras.get(id);
    // The host's own copy came: its block shows the run's state from now on, once a 전송 중 that has shown had its shortest
    // time (`holding`), which it keeps on that block meanwhile.
    if (!extra || (mighty.blocks[id] !== undefined && !holding(request))) return extras;
    const next = new Map(extras);
    next.set(id, { ...extra, pending: pendingState });
    return next;
  }, [extras, request, pendingState, pendingRunIdOf, mighty.blocks]);

  const visible = useMemo(() => new Set(visibleKey.split('\n')), [visibleKey]);
  // The step list over the diagram for a pane without the progress panel (an older host); the progress panel stands there instead.
  const steps = useMemo(() => (showsProgress ? null : stepsModel(session, runs)), [session, runs, showsProgress]);
  const progressCopy = inputs?.progress;
  const progressID = showsProgress && progressCopy ? progressRequest(pinnedRun, focusedRun, newestRequestID, (id) => progressCopy[id] !== undefined) : null;
  const progress = progressID !== null ? progressCopy?.[progressID] : undefined;
  panelShown.current = progress !== undefined;
  const unfinished = useMemo(() => new Set(runs.filter((r) => !finished(r)).map((r) => r.id)), [runs]);
  const toolbarZoom = (
    <span className="mighty-zoom">
      <button type="button" className="mighty-icon" title={t('tooltip.mighty.zoomOut')} aria-label={t('graph.zoom.out')} disabled={zoom <= ZoomMin} onClick={() => setZoomTo(zoomOut(zoom))}><Icon name="zoom-out" size={15} weight={1.75} /></button>
      <button type="button" className="mighty-zoom-label" title={t('tooltip.mighty.zoomReset')} onClick={() => setZoomTo(ZoomDefault)} data-testid="mighty-zoom">{zoomLabel(zoom)}</button>
      <button type="button" className="mighty-icon" title={t('tooltip.mighty.zoomIn')} aria-label={t('graph.zoom.in')} disabled={zoom >= ZoomMax} onClick={() => setZoomTo(zoomIn(zoom))}><Icon name="zoom-in" size={15} weight={1.75} /></button>
    </span>
  );

  return (
    <div className="mighty-diagram" ref={rootRef} data-testid="mighty-diagram" data-node-count={layout.nodes.length} data-edge-count={layout.edges.length}>
      <div className="mighty-diagram-zoom">{toolbarZoom}</div>
      <div className="mighty-viewport" ref={viewportRef} tabIndex={0} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerLeave={() => { if (!drag.current) showResizeCursor(''); }}
        onPointerUp={(e) => endDrag(e, false)} onPointerCancel={() => endDrag(null, true)} onLostPointerCapture={() => { if (drag.current || panning.current) endDrag(null, !!drag.current); }}
        onKeyDown={onKeyDown}>
        <svg className="mighty-dots-layer" ref={dotsLayer} aria-hidden="true">
          <defs>
            <pattern id={'mighty-dots-' + paneId} ref={dotsRef} patternUnits="userSpaceOnUse" width={18} height={18}>
              <circle ref={dotRef} cx={9} cy={9} r={1} className="mighty-dot" />
            </pattern>
          </defs>
          <rect ref={dotsRect} className="mighty-dots" width="100%" height="100%" fill={`url(#mighty-dots-${paneId})`} />
        </svg>
        {steps && <StepsPanel key={paneId} paneId={paneId} model={steps} t={t} />}
        {progress && <ProgressPanel key={paneId} paneId={paneId} progress={progress} onReveal={revealFlow} narrow={viewport.w > 0 && viewport.w < ProgressFoldWidth} />}
        <div className="mighty-world" ref={worldRef} onClick={onReferenceClick} onKeyDown={onReferenceKey}>
          {/* The edges, under the blocks and in their coordinates (the layout's extent from its origin): the world's transform moves them. */}
          <svg className="mighty-edges" style={{ left: edgesBox.x, top: edgesBox.y }} width={edgesBox.w} height={edgesBox.h} viewBox={`${edgesBox.x} ${edgesBox.y} ${edgesBox.w} ${edgesBox.h}`}
            aria-hidden="true" data-testid="mighty-edges">
            <path className="mighty-edges-quiet" d={edges.quiet} />
            <path className="mighty-edges-live" d={edges.live} />
          </svg>
          {layout.nodes.map((node) => {
            const copy = mighty.blocks[node.id] ?? copies?.get(node.id);
            const { x, y, w, h } = node.frame;
            // The role view's own nodes (FlowParts.tsx), each drawn from its copy; a node whose copy has not come yet waits for it.
            if (node.compact) {
              const card = copy?.flow?.card;
              return card ? <FlowCardNode key={node.id} id={node.id} kind={node.kind} x={x} y={y} w={w} h={h} card={card}
                outline={copy?.outline === 'marching' || copy?.outline === 'solid' || (card.running && node.kind !== 'agent')}
                onActivate={activateCard} onOpen={openFlowCard} /> : null;
            }
            if (node.kind === DelegateKind) return copy?.flow?.line ? <DelegateLineNode key={node.id} id={node.id} x={x} y={y} w={w} h={h} line={copy.flow.line} /> : null;
            return (
              <Block key={node.id} paneId={paneId} provider={session.provider} id={node.id} kind={node.kind}
                x={x} y={y} w={w} h={h}
                copy={copy} extra={drawnExtras.get(node.id) ?? EMPTY_EXTRA} visible={visible.has(node.id)}
                selected={selected === node.id} expanded={expanded.has(node.id)}
                running={unfinished.has(runOf(node.id) ?? '')}
                foldable={fanOut && copy?.flow?.card !== undefined}
                actions={actions} />
            );
          })}
        </div>
      </div>
      {/* The session log under a Claude pane's flow, above the composer (FlowLog): no strip without a request. */}
      {fanOut && inputs?.log && <FlowLogStrip paneId={paneId} log={inputs.log} squeezed={!logRowsFit(room)} onToggle={toggleLog} onReveal={revealFlow} />}
      {changesRun !== null && (
        <Suspense fallback={null}>
          <LazyRunChanges paneId={paneId} runId={changesRun} onClose={() => setChangesRun(null)} />
        </Suspense>
      )}
    </div>
  );
}


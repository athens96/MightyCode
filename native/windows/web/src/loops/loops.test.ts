import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Bridge } from '../bridge/bridge';
import { devHost } from '../bridge/devHost';
import { ServicesContext, type AppServices } from '../app/context';
import { isLoopShortcut, type EnterKey } from '../composer/keys';
import { busySendRows, isLoopChoice, loopRow, SendMenu } from '../composer/Toolbar';
import type { WorktreeAgent } from '../composer/types';
import { translator } from '../i18n/locale';
import { Sidebar } from '../shell/Sidebar';
import { macShortcutAction, shortcutAction } from '../shell/shortcuts';
import type { ShellModel } from '../shell/types';
import { Store } from '../store/store';
import { LoopBand } from './LoopBand';
import { LoopDetail } from './LoopDetail';
import { LoopTab } from './LoopTab';
import {
  activeLoopForPane, addCommandCriterion, buildLoopStartArgs, editDraftCriterionText, elapsedMinutes,
  firstFailingCriterion, formFromDraft, formatLimitUsd, formatMinutes, formatUsd, groupLoops,
  iterationProgress, loopCounts, loopGroup, loopMatches, loopRowText, meterFraction, parseCommandCheck,
  loopEntryAllowed, removeDraftCriterion, stateChip, toggleDraftCriterion,
} from './model';
import type { LoopRecord } from './types';

const t = translator([{
  'loop.ui.tab': '루프',
  'loop.ui.tabHelp': '루프 목록 ({key})',
  'loop.ui.tabWaiting': '{count}개가 나를 기다림',
  'loop.ui.tabRunning': '{count}개 도는 중',
  'loop.ui.group.waiting': '나를 기다림',
  'loop.ui.group.running': '도는 중',
  'loop.ui.group.finished': '끝남',
  'loop.ui.state.running': '도는 중',
  'loop.ui.state.paused': '잠시 멈춤',
  'loop.ui.state.pausing': '멈추는 중',
  'loop.ui.state.guard': '가드 경고',
  'loop.ui.state.passed': '통과',
  'loop.ui.state.failed': '실패',
  'loop.ui.state.stopped': '멈춤',
  'loop.ui.phase.build': '구현 중',
  'loop.ui.phase.verify': '확인 중',
  'loop.ui.phase.repair': '고치는 중',
  'loop.ui.phase.done': '끝남',
  'loop.ui.stop.passed': '모두 통과',
  'loop.ui.stop.maxIterations': '최대 반복 도달',
  'loop.ui.stop.maxMinutes': '시간 상한 도달',
  'loop.ui.stop.maxCost': '비용 상한 도달',
  'loop.ui.stop.budget': '하루 예산 도달',
  'loop.ui.stop.sameFailure': '같은 실패 반복',
  'loop.ui.stop.person': '직접 멈춤',
  'loop.ui.stop.repairEnded': '수리 턴 종료',
  'loop.ui.stop.paneClosed': '창이 닫힘',
  'loop.ui.guard.testsRemoved': '테스트 삭제됨',
  'loop.ui.guard.skipAdded': '건너뛰기 추가됨',
  'loop.ui.guard.protectedChanged': '보호 파일 변경됨',
  'loop.ui.guard.clean': '없음',
  'loop.ui.guard.flagCount': '{count}건',
  'loop.ui.guard.title': '가드 경고',
  'loop.ui.guard.protectedFiles': '보호 파일 {count}개',
  'loop.ui.guard.tests': '테스트 {start} → {now}',
  'loop.ui.guard.skips': 'skip 추가 {count}건',
  'loop.ui.guard.touched': '건드린 파일',
  'loop.ui.iterationRatio': '반복 {n}/{max}',
  'loop.ui.iterationNumber': '반복 {n}',
  'loop.ui.iteration.changedFiles': '바뀐 파일 {count}개',
  'loop.ui.row.failing': '{name} 실패',
  'loop.ui.row.pausedHint': '계속할 수 있음',
  'loop.ui.duration.minutes': '{n}분',
  'loop.ui.duration.hours': '{n}시간',
  'loop.ui.duration.hoursMinutes': '{h}시간 {m}분',
  'loop.ui.action.pause': '잠시 멈춤',
  'loop.ui.action.pauseHelp': '현재 턴과 확인이 끝나면 멈춥니다',
  'loop.ui.action.resume': '계속',
  'loop.ui.action.resumeHelp': '확인을 먼저 돌린 뒤 필요하면 고칩니다',
  'loop.ui.action.stop': '멈추기',
  'loop.ui.action.stopHelp': '지금 바로 멈춥니다',
  'loop.ui.action.forget': '목록에서 지우기',
  'loop.ui.action.viewInPane': '창에서 보기',
  'loop.ui.action.closeDetail': '닫기',
  'loop.ui.band.label': '루프',
  'loop.ui.band.view': '루프 보기',
  'loop.ui.sendMenu': '될 때까지 반복…',
  'loop.ui.sendMenuHelp': '조건이 모두 통과할 때까지 자동으로 고치고 확인합니다',
  'loop.ui.criteria.label': '확인',
  'loop.ui.kind.build': '구현',
  'loop.ui.kind.repair': '수리',
  'loop.ui.outcome.running': '도는 중',
  'loop.ui.outcome.passed': '통과',
  'loop.ui.outcome.failed': '실패',
  'loop.ui.outcome.stopped': '멈춤',
  'loop.ui.stat.iterations': '반복',
  'loop.ui.stat.cost': '비용',
  'loop.ui.stat.elapsed': '시간',
  'loop.ui.stat.guard': '가드',
  'loop.ui.startButton': '루프 시작',
  'loop.ui.empty': '아직 루프가 없습니다.',
  'loop.ui.timeline.empty': '아직 실행된 반복이 없습니다.',
  'loop.ui.timeline.started': '시작 · {at}',
  'loop.ui.detail.baseCommit': '기준 커밋 {sha}',
  'loop.ui.detail.pane': '창: {pane}',
  'compare.menu': '비교 실행…',
  'compare.menuHelp': '여러 에이전트에서 나란히 실행',
  'composer.busySend.title': '실행 중 보내기',
  'composer.busySend.queue': '대기열',
  'composer.busySend.worktree': '워크트리 · {agent}',
}]);

const makeLoop = (over: Partial<LoopRecord> = {}): LoopRecord => ({
  id: 'loop-1',
  paneId: 'p1',
  workspaceId: 'w1',
  title: 'CI 초록불까지',
  goal: 'Fix all failing tests',
  state: 'running',
  phase: 'verify',
  criteria: [
    { id: 'c1', text: '타입 검사', check: { kind: 'verifyConfig', name: 'typecheck', command: 'tsc' }, level: 'L1', status: 'passed', evidence: null },
    { id: 'c2', text: '단위 테스트', check: { kind: 'command', name: 'test', command: 'npm test' }, level: 'L2', status: 'failed', evidence: { level: 'L2', summary: '12 passed, 1 failed' } },
  ],
  criteriaHash: 'ab12cd34',
  limits: { maxIterations: 10, maxMinutes: 120, maxCostUsd: 5 },
  iterations: [
    { n: 1, kind: 'build', runId: 'run-1', startedAt: '2026-11-28T00:00:00Z', finishedAt: '2026-11-28T00:04:00Z', outcome: 'failed', failed: ['c1', 'c2'], changedFiles: 3, added: 24, removed: 5, costUsd: 0.55 },
    { n: 2, kind: 'repair', runId: 'run-2', startedAt: '2026-11-28T00:04:00Z', finishedAt: '2026-11-28T00:09:00Z', outcome: 'failed', failed: ['c2'], changedFiles: 2, added: 18, removed: 6, costUsd: 0.65 },
  ],
  guard: { protectedFiles: 42, touched: [], testsAtStart: 310, testsNow: 310, skipsAdded: 0, flags: [] },
  costUsd: 1.2,
  startedAt: '2026-11-28T00:00:00Z',
  finishedAt: null,
  stopReason: null,
  baseCommit: 'a91c2e0f',
  ...over,
});

describe('loop grouping and counts (MC-119)', () => {
  const running = makeLoop({ id: 'r1', state: 'running' });
  const paused = makeLoop({ id: 'p1', state: 'paused', title: 'PR 리뷰 반영' });
  const guarded = makeLoop({
    id: 'g1',
    state: 'running',
    title: '테스트 보호 경고',
    guard: { protectedFiles: 10, touched: ['a.test.ts'], testsAtStart: 20, testsNow: 19, skipsAdded: 0, flags: ['protectedChanged', 'testsRemoved'] },
  });
  const passed = makeLoop({ id: 'd1', state: 'passed', stopReason: 'passed' });
  const failedWithFlag = makeLoop({
    id: 'f1',
    state: 'failed',
    stopReason: 'sameFailure',
    guard: { protectedFiles: 10, touched: [], testsAtStart: 20, testsNow: 19, skipsAdded: 0, flags: ['testsRemoved'] },
  });
  const stopped = makeLoop({ id: 's1', state: 'stopped', stopReason: 'person' });
  const draft = makeLoop({ id: 'dr1', state: 'draft' });

  it('groups paused loops under waiting, running ones (guard flags or not) under running, terminal states under finished', () => {
    expect(loopGroup(paused)).toBe('waiting');
    expect(loopGroup(guarded)).toBe('running');
    expect(loopGroup(running)).toBe('running');
    expect(loopGroup(passed)).toBe('finished');
    expect(loopGroup(failedWithFlag)).toBe('finished');
    expect(loopGroup(stopped)).toBe('finished');
    expect(loopGroup(draft)).toBeNull();

    const groups = groupLoops([running, paused, guarded, passed, failedWithFlag, stopped, draft]);
    expect(groups.map((g) => [g.group, g.loops.map((l) => l.id)])).toEqual([
      ['waiting', ['p1']],
      ['running', ['r1', 'g1']],
      ['finished', ['d1', 'f1', 's1']],
    ]);
  });

  it('omits empty groups and tallies counts accurately', () => {
    expect(groupLoops([running]).map((g) => g.group)).toEqual(['running']);
    expect(loopCounts([running, paused, guarded, passed, stopped, draft])).toEqual({
      waiting: 1,
      running: 2,
      finished: 2,
      total: 5,
    });
  });

  it('matches sidebar search against title, goal, stop reason, and criteria', () => {
    expect(loopMatches(running, '초록')).toBe(true);
    expect(loopMatches(running, 'npm test')).toBe(true);
    expect(loopMatches(failedWithFlag, 'samefailure')).toBe(true);
    expect(loopMatches(running, 'unrelated-xyz')).toBe(false);
  });

  it('finds the active loop (running or paused) for a pane from the loops list', () => {
    const loops = [
      makeLoop({ id: 'done', paneId: 'p1', state: 'passed' }),
      makeLoop({ id: 'active', paneId: 'p1', state: 'paused' }),
      makeLoop({ id: 'other', paneId: 'p2', state: 'running' }),
    ];
    expect(activeLoopForPane(loops, 'p1')?.id).toBe('active');
    expect(activeLoopForPane(loops, 'p2')?.id).toBe('other');
    expect(activeLoopForPane(loops, 'p3')).toBeUndefined();
    expect(activeLoopForPane([makeLoop({ id: 'done', paneId: 'p1', state: 'stopped' })], 'p1')).toBeUndefined();
  });
});

describe('loop row text and formatters (MC-119)', () => {
  it('formats running row with iteration ratio, first failing criterion, and cost', () => {
    const loop = makeLoop();
    expect(iterationProgress(loop)).toEqual({ current: 2, max: 10 });
    expect(firstFailingCriterion(loop)?.id).toBe('c2');
    expect(loopRowText(loop, t)).toEqual({
      iterations: '반복 2/10',
      detail: '반복 2/10 · 단위 테스트 실패',
      cost: '$1.20 / $5',
    });
    expect(stateChip(loop, t)).toEqual({ label: '도는 중', tone: 'run' });
  });

  it('highlights guard flags and paused state on waiting rows', () => {
    const guarded = makeLoop({
      state: 'running',
      guard: { protectedFiles: 12, touched: ['a.test.ts'], testsAtStart: 50, testsNow: 49, skipsAdded: 0, flags: ['protectedChanged'] },
    });
    expect(loopRowText(guarded, t).detail).toBe('반복 2/10 · 보호 파일 변경됨');
    expect(stateChip(guarded, t)).toEqual({ label: '가드 경고', tone: 'wait' });

    const paused = makeLoop({ state: 'paused' });
    expect(loopRowText(paused, t).detail).toBe('반복 2/10 · 계속할 수 있음');
    expect(stateChip(paused, t)).toEqual({ label: '잠시 멈춤', tone: 'wait' });
  });

  it('shows stop reason on failed/stopped rows and spent cost on passed rows', () => {
    const passed = makeLoop({ state: 'passed', stopReason: 'passed', costUsd: 0.8 });
    expect(loopRowText(passed, t).detail).toBe('반복 2/10 · $0.80');
    expect(stateChip(passed, t)).toEqual({ label: '통과', tone: 'done' });

    const failed = makeLoop({ state: 'failed', stopReason: 'sameFailure' });
    expect(loopRowText(failed, t).detail).toBe('반복 2/10 · 같은 실패 반복');
    expect(stateChip(failed, t)).toEqual({ label: '실패', tone: 'err' });
  });

  it('formats meters, USD, and elapsed minutes cleanly', () => {
    expect(meterFraction(1.2, 5)).toBeCloseTo(0.24);
    expect(meterFraction(8, 5)).toBe(1);
    expect(meterFraction(1, 0)).toBe(0);
    expect(formatUsd(1.2)).toBe('$1.20');
    expect(formatLimitUsd(5)).toBe('$5');
    expect(formatLimitUsd(5.5)).toBe('$5.50');
    expect(formatMinutes(12, t)).toBe('12분');
    expect(formatMinutes(120, t)).toBe('2시간');
    expect(formatMinutes(95, t)).toBe('1시간 35분');
    expect(elapsedMinutes('2026-11-28T00:00:00Z', '2026-11-28T00:15:00Z')).toBe(15);
  });
});

describe('loop create-sheet logic (MC-119)', () => {
  const draft = makeLoop({
    id: 'draft',
    state: 'draft',
    goal: 'Draft goal',
    criteria: [
      { id: 'c1', text: 'Typecheck', check: { kind: 'verifyConfig', name: 'typecheck', command: 'tsc' }, level: 'L1', status: 'pending', evidence: null },
      { id: 'c2', text: 'Checklist item', check: { kind: 'checklist', name: 'check' }, level: 'L1', status: 'pending', evidence: null },
    ],
    limits: { maxIterations: 10, maxMinutes: 120, maxCostUsd: 5 },
  });

  it('prefills composer goal over draft goal and enables all drafted criteria', () => {
    const form = formFromDraft(draft, 'Fix CI on PR #214');
    expect(form.goal).toBe('Fix CI on PR #214');
    expect(form.criteria.map((c) => [c.id, c.enabled, c.text])).toEqual([
      ['c1', true, 'Typecheck'],
      ['c2', true, 'Checklist item'],
    ]);
    expect(form.maxIterations).toBe('10');
    expect(form.maxMinutes).toBe('120');
    expect(form.maxCostUsd).toBe('5');

    expect(formFromDraft(draft, '   ').goal).toBe('Draft goal');
  });

  it('parses name=command checks, preserving extra = signs in the command', () => {
    expect(parseCommandCheck('test=npm test')).toEqual({ name: 'test', command: 'npm test' });
    expect(parseCommandCheck(' lint = NODE_ENV=test npm run lint ')).toEqual({ name: 'lint', command: 'NODE_ENV=test npm run lint' });
    expect(parseCommandCheck('noequals')).toBeNull();
    expect(parseCommandCheck('=npm test')).toBeNull();
    expect(parseCommandCheck('test=   ')).toBeNull();
  });

  it('toggles, edits, adds, and removes criteria immutably', () => {
    let form = formFromDraft(draft, 'Goal');
    form = toggleDraftCriterion(form, 'c1');
    expect(form.criteria[0]?.enabled).toBe(false);
    form = editDraftCriterionText(form, 'c2', 'Updated checklist');
    expect(form.criteria[1]?.text).toBe('Updated checklist');

    const bad = addCommandCriterion(form, 'invalid');
    expect(bad.ok).toBe(false);

    const added = addCommandCriterion(form, 'e2e=npm run e2e', 'L4');
    expect(added.ok).toBe(true);
    if (added.ok) {
      expect(added.form.criteria).toHaveLength(3);
      const custom = added.form.criteria[2]!;
      expect(custom.check).toEqual({ kind: 'command', name: 'e2e', command: 'npm run e2e', cwd: null });
      expect(custom.level).toBe('L4');
      expect(custom.custom).toBe(true);

      const removed = removeDraftCriterion(added.form, custom.id);
      expect(removed.criteria).toHaveLength(2);
    }
  });

  it('validates goal, executable criterion requirement, and numeric limits when building loopStart args', () => {
    const base = formFromDraft(draft, 'Ship it');

    // Empty goal rejected
    expect(buildLoopStartArgs('p1', { ...base, goal: '  ' })).toEqual({ ok: false, errorKey: 'loop.ui.create.goalRequired' });

    // Checklist alone (no command or verifyConfig) rejected
    const checklistOnly = toggleDraftCriterion(base, 'c1');
    expect(buildLoopStartArgs('p1', checklistOnly)).toEqual({ ok: false, errorKey: 'loop.ui.create.commandRequired' });

    // Invalid limits rejected
    expect(buildLoopStartArgs('p1', { ...base, maxIterations: '0' })).toEqual({ ok: false, errorKey: 'loop.ui.create.limitsInvalid' });
    expect(buildLoopStartArgs('p1', { ...base, maxMinutes: '1.5' })).toEqual({ ok: false, errorKey: 'loop.ui.create.limitsInvalid' });
    expect(buildLoopStartArgs('p1', { ...base, maxCostUsd: '-1' })).toEqual({ ok: false, errorKey: 'loop.ui.create.limitsInvalid' });

    // Valid payload
    const res = buildLoopStartArgs('p1', { ...base, maxCostUsd: '7.5' });
    expect(res.ok).toBe(true);
    if (res.ok) {
      expect(res.args.paneId).toBe('p1');
      expect(res.args.goal).toBe('Ship it');
      expect(res.args.criteria).toHaveLength(2);
      expect(res.args.limits).toEqual({ maxIterations: 10, maxMinutes: 120, maxCostUsd: 7.5 });
    }
  });
});

describe('shortcuts, send menu, store push, and rendering (MC-119)', () => {
  const enter = (over: Partial<EnterKey> = {}): EnterKey => ({
    key: 'Enter', isComposing: false, keyCode: 13, shiftKey: false, altKey: false, metaKey: false, ctrlKey: false, repeat: false, ...over,
  });

  it('maps ⌘3 / Ctrl+3 to tabLoops and ⌥⌘Enter / Ctrl+Alt+Enter to loop create shortcut', () => {
    expect(shortcutAction({ key: '3', code: 'Digit3', ctrlKey: true, shiftKey: false, altKey: false, metaKey: false })).toBe('tabLoops');
    expect(macShortcutAction({ key: '3', code: 'Digit3', ctrlKey: false, shiftKey: false, altKey: false, metaKey: true })).toBe('tabLoops');

    expect(isLoopShortcut(enter({ metaKey: true, altKey: true }), true)).toBe(true);
    expect(isLoopShortcut(enter({ ctrlKey: true, altKey: true }), true)).toBe(false);
    expect(isLoopShortcut(enter({ ctrlKey: true, altKey: true }), false)).toBe(true);
    expect(isLoopShortcut(enter({ metaKey: true, altKey: true }), false)).toBe(false);
    expect(isLoopShortcut(enter({ ctrlKey: true, altKey: true, repeat: true }), false)).toBe(false);
    expect(isLoopShortcut(enter({ ctrlKey: true, altKey: true, isComposing: true }), false)).toBe(false);
  });

  it('adds ↻ 될 때까지 반복… to the send menus when enabled', () => {
    const agents: WorktreeAgent[] = [{ agent: 'claude', label: 'Claude', detail: 'Claude', enabled: true }];
    const rows = busySendRows(agents, null, t, true, true, true);
    expect(rows[rows.length - 1]).toEqual(loopRow(t, true));
    expect(isLoopChoice(agents, rows.length - 1, true)).toBe(true);
    const winRow = loopRow(t, false);
    const macRow = loopRow(t, true);
    expect(winRow.kind === 'action' ? winRow.shortcut : '').toBe('Ctrl+Alt+Enter');
    expect(macRow.kind === 'action' ? macRow.shortcut : '').toBe('⌥⌘Enter');

    const services = { bridge: { call: () => Promise.resolve(null) }, store: new Store(), t } as unknown as AppServices;
    const html = renderToStaticMarkup(
      createElement(ServicesContext.Provider, { value: services },
        createElement(SendMenu, { onCompare: () => {}, onLoop: () => {}, mac: true, onClose: () => {} })),
    );
    expect(html).toContain('↻ 될 때까지 반복…');
    expect(html).toContain('⌥⌘Enter');
  });

  it('handles loops push in the store and renders Sidebar tab, LoopTab, LoopDetail, and LoopBand', () => {
    const store = new Store();
    const active = makeLoop({ id: 'loop-run', paneId: 'p1', state: 'running' });
    const guarded = makeLoop({
      id: 'loop-wait',
      paneId: 'p2',
      state: 'paused',
      title: '리뷰 반영',
      guard: { protectedFiles: 12, touched: ['src/foo.test.ts'], testsAtStart: 100, testsNow: 99, skipsAdded: 1, flags: ['protectedChanged', 'testsRemoved'] },
    });
    const counts = { questions: 0, permissions: 0, errors: 0, running: 1, done: 0, stopped: 0, idle: 0 };
    store.dispatch({
      kind: 'snapshot',
      snapshot: { workspaces: [], sessions: [{ id: 'p1', title: 'Main pane', kind: 'claude' }], theme: 'dark' } as unknown as Parameters<Store['dispatch']>[0] extends { kind: 'snapshot'; snapshot: infer S } ? S : never,
    });
    store.dispatch({
      kind: 'shell',
      shell: {
        version: '0.7.0', dialogOpen: false, panes: [], entry: counts, workspaces: [],
        status: { machine: '', panes: '1', running: '1', updateHelp: '', setup: false, chipsTitle: '', chipsTooltip: '' },
        setupNeeded: false,
      } as unknown as ShellModel,
    });

    const services = { bridge: { call: () => Promise.resolve(null) }, store, t } as unknown as AppServices;

    // Disabled by default: third sidebar tab and pane band stay hidden
    const sidebarOff = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(Sidebar)));
    expect(sidebarOff).not.toContain('data-testid="sidebar-tab-loops"');
    const bandOff = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(LoopBand, { paneId: 'p1' })));
    expect(bandOff).toBe('');

    // Enable loops and push records
    store.dispatch({ kind: 'loops', enabled: true, loops: [active, guarded] });
    expect(store.getState().loops.enabled).toBe(true);
    expect(store.getState().loops.loops).toHaveLength(2);

    const sidebarOn = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(Sidebar)));
    expect(sidebarOn).toContain('data-testid="sidebar-tab-loops"');
    expect(sidebarOn).toContain('data-testid="sidebar-tab-loops-badge"');

    const tabHtml = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(LoopTab, { search: '', selectedLoopId: 'loop-run' })));
    expect(tabHtml).toContain('data-testid="sidebar-loop-group-waiting"');
    expect(tabHtml).toContain('data-testid="sidebar-loop-group-running"');
    expect(tabHtml).toContain('CI 초록불까지');
    expect(tabHtml).toContain('리뷰 반영');

    const detailHtml = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(LoopDetail, { loop: guarded })));
    expect(detailHtml).toContain('data-testid="loop-guard-card"');
    expect(detailHtml).toContain('src/foo.test.ts');
    expect(detailHtml).toContain('L1');
    expect(detailHtml).toContain('L2');
    expect(detailHtml).toContain('창에서 보기');

    const bandOn = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(LoopBand, { paneId: 'p1' })));
    expect(bandOn).toContain('data-testid="pane-loop-band"');
    expect(bandOn).toContain('CI 초록불까지');
    expect(bandOn).toContain('반복 2/10');
    expect(bandOn).toContain('단위 테스트');
    expect(bandOn).toContain('루프 보기');
    expect(bandOn).toContain('멈추기');
  });

  it('serves fake loops and loop actions in devHost', async () => {
    const bridge = new Bridge(devHost());
    const list = await bridge.call<{ enabled: boolean; loops: LoopRecord[] }>('loopList', {});
    expect(list.enabled).toBe(false);
    await bridge.call('loopSetEnabled', { on: true });
    expect(list.loops.length).toBeGreaterThanOrEqual(2);

    const draft = await bridge.call<LoopRecord>('loopDraft', { paneId: 's1', goal: 'Fix lint' });
    expect(draft.state).toBe('draft');
    expect(draft.criteria.length).toBeGreaterThanOrEqual(1);

    const pausing = await bridge.call<LoopRecord>('loopPause', { id: 'loop-ci-214' });
    expect([pausing.state, pausing.pausing]).toEqual(['running', true]);
    const resumed = await bridge.call<LoopRecord>('loopResume', { id: 'loop-ci-214' });
    expect([resumed.state, resumed.pausing]).toEqual(['running', false]);
    const stopped = await bridge.call<LoopRecord>('loopStop', { id: 'loop-ci-214' });
    expect(stopped.state).toBe('stopped');
    await bridge.call('loopForget', { id: 'loop-ci-214' });
    const after = await bridge.call<{ enabled: boolean; loops: LoopRecord[] }>('loopList', {});
    expect(after.loops.some((l) => l.id === 'loop-ci-214')).toBe(false);
  });

  // PR #35 review: the gates and edges the first cut left untested.
  describe('review follow-ups', () => {
    it('offers the loop entry only where the host would start a loop', () => {
      const pane = { loopsEnabled: true, kind: 'claude', hasPills: true, parentSessionId: null, busy: false, hasLoop: false };
      expect(loopEntryAllowed(pane)).toBe(true);
      expect(loopEntryAllowed({ ...pane, loopsEnabled: false })).toBe(false);
      expect(loopEntryAllowed({ ...pane, kind: 'shell' })).toBe(false);
      expect(loopEntryAllowed({ ...pane, parentSessionId: 'parent' })).toBe(false);
      expect(loopEntryAllowed({ ...pane, busy: true })).toBe(false);
      expect(loopEntryAllowed({ ...pane, hasLoop: true })).toBe(false);
      expect(loopEntryAllowed({ ...pane, hasPills: false })).toBe(false);
    });

    it("refuses limits outside the host's ranges and empty ones", () => {
      const draft = makeLoop({ state: 'draft' });
      const base = formFromDraft(draft, 'Goal');
      const args = (over: Partial<typeof base>) => buildLoopStartArgs('p1', { ...base, ...over });
      expect(args({}).ok).toBe(true);
      for (const over of [
        { maxIterations: '' }, { maxIterations: '0' }, { maxIterations: '51' }, { maxIterations: '5000000000' }, { maxIterations: '2.5' },
        { maxMinutes: '' }, { maxMinutes: '1441' }, { maxCostUsd: '' }, { maxCostUsd: '0.05' }, { maxCostUsd: '501' }, { maxCostUsd: 'abc' },
      ]) {
        expect(args(over), JSON.stringify(over)).toEqual({ ok: false, errorKey: 'loop.ui.create.limitsInvalid' });
      }
      expect(args({ maxIterations: '50', maxMinutes: '1440', maxCostUsd: '500' }).ok).toBe(true);
      expect(args({ maxIterations: '1', maxMinutes: '1', maxCostUsd: '0.1' }).ok).toBe(true);
    });

    it('keeps the switch when a push leaves enabled out', () => {
      const store = new Store();
      store.dispatch({ kind: 'loops', enabled: true, loops: [] });
      store.dispatch({ kind: 'loops', loops: [makeLoop()] } as unknown as Parameters<Store['dispatch']>[0]);
      expect(store.getState().loops.enabled).toBe(true);
      expect(store.getState().loops.loops).toHaveLength(1);
    });

    it('shows a requested pause as pausing until the loop really pauses', () => {
      expect(stateChip(makeLoop({ state: 'running', pausing: true }), t)).toEqual({ label: '멈추는 중', tone: 'wait' });
      expect(stateChip(makeLoop({ state: 'paused', pausing: false }), t).label).toBe('잠시 멈춤');
    });
  });
});

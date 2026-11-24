import type { MergeChecks, MobileSessionSummary, MobileState } from '@/api/types';
import { CAPABILITIES } from '@/api/types';
import { hasCapability, parseCapabilities } from '@/lib/capabilities';
import {
  changesWords,
  columnAgent,
  compareRequestLine,
  compareRunsOf,
  elapsedWords,
  phaseCounts,
  summaryColumn,
  testWords,
  usd,
} from '@/lib/compare';
import { resetLanguage } from '@/lib/i18n';
import { checksSections, mergeConfirmText, parseWarnings, warningLine } from '@/lib/merge-checks';
import {
  attentionLine,
  countMarked,
  forkSource,
  latestSandboxBlock,
  newestMarked,
  verifyLine,
} from '@/lib/pane-marks';
import { worktreeActions, worktreePortBlock } from '@/lib/pane-worktree';

beforeAll(() => resetLanguage('ko'));
afterAll(() => resetLanguage());

function pane(overrides: Partial<MobileSessionSummary>): MobileSessionSummary {
  return {
    id: 's1',
    workspaceId: 'w1',
    title: '창',
    kind: 'claude',
    provider: 'claude',
    model: 'opus',
    status: 'idle',
    revision: 1,
    updatedAt: '2026-11-23T00:00:00.000Z',
    pendingPermissions: 0,
    pendingQuestions: 0,
    queued: 0,
    terminal: false,
    ...overrides,
  };
}

function state(sessions: MobileSessionSummary[]): MobileState {
  return { protocol: 1, revision: 1, hostName: 'mac', workspaces: [], sessions };
}

describe('the 0.6.0 capabilities', () => {
  it('turns the checks and the compare view on only where the host lists them', () => {
    expect(CAPABILITIES).toEqual(expect.arrayContaining(['mergeChecks', 'compare']));
    const listed = parseCapabilities({ capabilities: ['pane', 'mergeChecks', 'compare'] });
    expect(hasCapability(listed, 'mergeChecks') && hasCapability(listed, 'compare')).toBe(true);
    const older = parseCapabilities({ capabilities: ['pane', 'savedPrompts'] });
    expect(hasCapability(older, 'mergeChecks') || hasCapability(older, 'compare')).toBe(false);
  });
});

describe('MC-25: the worktree bar’s port block and run actions', () => {
  const base = { branch: 'mighty/p1', base: 'main', port: 4321 };

  it('writes the block as the desktop does, and nothing without one', () => {
    expect(worktreePortBlock({ ...base, portBase: 41010, portCount: 10 })).toBe(':41010–41019');
    expect(worktreePortBlock({ ...base, portBase: 41010, portCount: 1 })).toBe(':41010');
    expect(worktreePortBlock(base)).toBeUndefined();
    expect(worktreePortBlock({ ...base, portBase: 41010 })).toBeUndefined();
  });

  it('keeps the running actions with a web address only', () => {
    expect(
      worktreeActions({
        ...base,
        actions: [
          { name: 'dev', url: 'http://localhost:41010/' },
          { name: 'e2e', url: 'javascript:alert(1)' },
          { name: '' },
        ],
      }),
    ).toEqual([{ name: 'dev', url: 'http://localhost:41010/' }, { name: 'e2e' }]);
    expect(worktreeActions(base)).toEqual([]);
  });
});

describe('MC-27: the pre-merge checks', () => {
  it('words the warnings the host sends back with a merge', () => {
    const warnings = parseWarnings([{ code: 'checksFailed', count: 2 }, { code: 'threads', count: 1 }, { bad: true }, null]);
    expect(warnings).toEqual([
      { code: 'checksFailed', count: 2 },
      { code: 'threads', count: 1 },
    ]);
    expect(warningLine(warnings[0]!)).toBe('실패한 CI 검사 2개');
    expect(mergeConfirmText(warnings)).toBe(
      '아래 항목이 남아 있습니다. 그래도 병합할까요?\n• 실패한 CI 검사 2개\n• 해결 안 된 리뷰 스레드 1개',
    );
    expect(parseWarnings(undefined)).toEqual([]);
  });

  it('draws every part on its own, one not read saying why', () => {
    const model: MergeChecks = {
      protocol: 1,
      git: { ok: true, value: { head: 'mighty/p1', base: 'main', uncommitted: 2, conflicted: 0, ahead: 3, behind: 1 } },
      pr: {
        ok: true,
        value: {
          hosting: 'github',
          number: 42,
          url: 'https://github.com/o/r/pull/42',
          state: 'open',
          title: 'Fix login',
          isDraft: true,
          mergeable: 'conflicting',
          reviewDecision: 'changesRequested',
        },
      },
      checks: {
        ok: true,
        value: [
          { name: 'build', state: 'passed', url: '' },
          { name: 'test', state: 'failed', url: '' },
        ],
      },
      threads: { ok: false, reason: 'notLoggedIn', cli: 'gh' },
      checklist: { ok: true, value: { total: 3, remaining: ['문서'] } },
      warnings: [],
    };
    const sections = checksSections(model);
    expect(sections.map((section) => [section.id, section.tone])).toEqual([
      ['git', 'err'],
      ['pr', 'err'],
      ['ci', 'err'],
      ['threads', 'wait'],
      ['checklist', 'err'],
    ]);
    expect(sections[0]!.lines).toEqual(['커밋 안 된 변경 2개', 'main 기준 ↑3 ↓1']);
    expect(sections[1]!.lines).toEqual(['PR #42 · 열림 · 초안', 'Fix login', '충돌 있음', '변경 요청됨']);
    expect(sections[2]!.lines).toEqual(['통과 1 · 실패 1 · 진행 중 0', '✕ test']);
    expect(sections[3]!.lines[0]).toBe('확인할 수 없음 · gh에 로그인하지 않았습니다.');
    expect(sections[4]!.lines).toEqual(['남은 할 일 1개', '☐ 문서']);
  });
});

describe('MC-29: compare runs', () => {
  const run = {
    id: 'cmp1',
    request: '\n  Make it faster\nmore',
    baseCommit: 'abc1234',
    test: 'run:test',
    children: [
      { id: 'c1', provider: 'claude', model: 'opus' },
      { id: 'c2', provider: 'codex', failure: 'signed out' },
      { id: 'c3', provider: 'claude' },
    ],
  };

  it('reads the runs newest first and the request’s first line', () => {
    const parent = pane({ compareRuns: [{ ...run, id: 'old' }, run] });
    expect(compareRunsOf(parent).map((entry) => entry.id)).toEqual(['cmp1', 'old']);
    expect(compareRunsOf(pane({}))).toEqual([]);
    expect(compareRequestLine(run)).toBe('Make it faster');
  });

  it('tells each child’s state from its pane until the view is read', () => {
    const panes = [pane({ id: 'c1', childState: 'reported' }), pane({ id: 'c3', childState: 'running' })];
    const columns = run.children.map((child) => summaryColumn(child, panes));
    expect(columns.map((column) => column.phase)).toEqual(['reported', 'failed', 'running']);
    expect(columns[0]!.canOpen).toBe(true);
    expect(columns[1]!.canOpen).toBe(false);
    expect(phaseCounts(columns)).toBe('실행 중 1 · 보고 1 · 실패 1');
  });

  it('words a column as the desktop’s compare view does', () => {
    expect(changesWords({ files: 3, added: 12, removed: 4 })).toBe('파일 3개 +12 −4');
    expect(changesWords({ files: 0 })).toBe('변경 없음');
    expect(changesWords({ files: null })).toBe('—');
    expect(elapsedWords(125)).toBe('2:05');
    expect(elapsedWords(3725)).toBe('1:02:05');
    expect(elapsedWords(null)).toBe('—');
    expect(usd(0.4213)).toBe('$0.42');
    expect(usd(undefined)).toBe('—');
    expect(testWords(undefined, undefined, false)).toBe('테스트 없음');
    expect(testWords(undefined, undefined, true)).toBe('보고 후 실행');
    expect(testWords('failed', 2, true)).toBe('실패(코드 2)');
    expect(columnAgent({ agent: 'codex', agentLabel: 'Codex', model: null })).toBe('Codex · 기본 모델');
  });
});

describe('MC-26, MC-28, MC-34: the pane marks', () => {
  it('counts the marked panes and finds the one marked last', () => {
    const a = pane({ id: 'a', attention: { kind: 'bell', text: '', at: '2026-11-23T01:00:00Z' } });
    const b = pane({ id: 'b', attention: { kind: 'approval', text: 'Bash 승인', at: '2026-11-23T02:00:00Z' } });
    const own = pane({ id: 'agent-terminal:x', kind: 'agent-terminal', attention: { kind: 'bell', text: '', at: '2026-11-23T03:00:00Z' } });
    const states = { h1: state([a, own]), h2: state([b, pane({ id: 'c' })]) };
    expect(countMarked(states.h1)).toBe(1);
    expect(countMarked(states.h2)).toBe(1);
    expect(countMarked(undefined)).toBe(0);
    expect(newestMarked([{ id: 'h1' }, { id: 'h2' }], states)).toEqual({ hostId: 'h2', sessionId: 'b' });
    expect(newestMarked([{ id: 'h1' }], { h1: state([pane({})]) })).toBeUndefined();
    expect(attentionLine(a)).toBe('벨');
    expect(attentionLine(b)).toBe('승인 대기 · Bash 승인');
    expect(attentionLine(pane({}))).toBeUndefined();
  });

  it('links a fork to its source and names a closed one', () => {
    const source = pane({ id: 'src', title: '로그인 고치기' });
    const fork = pane({ id: 'f', forkedFrom: { paneId: 'src', title: '예전 제목' } });
    expect(forkSource(fork, [source, fork])).toEqual({ paneId: 'src', title: '로그인 고치기', open: true });
    expect(forkSource(fork, [fork])).toEqual({ paneId: 'src', title: '예전 제목', open: false });
    expect(forkSource(source, [source])).toBeUndefined();
  });

  it('says the latest sandbox refusal in the desktop’s words', () => {
    expect(
      latestSandboxBlock({
        sandboxBlocked: [
          { kind: 'network', what: 'a.com:443', reason: '처음', at: '' },
          { kind: 'network', what: 'example.com:443', reason: '허용 목록에 없는 곳입니다.', at: '' },
        ],
      }),
    ).toBe('샌드박스가 막았습니다: example.com:443 — 허용 목록에 없는 곳입니다.');
    expect(latestSandboxBlock({ sandboxBlocked: [{ kind: 'tool', what: 'rm -rf', reason: '위험', at: '' }] })).toBe(
      '심사 에이전트가 거부했습니다: rm -rf — 위험',
    );
    expect(latestSandboxBlock({})).toBeUndefined();
  });

  it('says the newest verification (MC-48) in one line, and nothing from an older host', () => {
    expect(verifyLine({ verify: { status: 'passed', rounds: 1 } })).toEqual({ text: '검증 통과', tone: 'done' });
    expect(
      verifyLine({
        verify: {
          status: 'failed',
          rounds: 3,
          stopReason: 'maxRounds',
          commands: [
            { name: 'typecheck', passed: true },
            { name: 'test', passed: false },
          ],
        },
      }),
    ).toEqual({ text: '검증 실패 (3회 시도) · test · 자동 수리 횟수 상한에 도달했습니다.', tone: 'fail' });
    expect(verifyLine({ verify: { status: 'repairing', rounds: 1 } })?.tone).toBe('run');
    expect(verifyLine({})).toBeUndefined();
  });
});

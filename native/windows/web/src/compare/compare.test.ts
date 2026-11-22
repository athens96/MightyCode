import { describe, expect, it } from 'vitest';
import { translator } from '../i18n/locale';
import { busySendRows, isCompareChoice } from '../composer/Toolbar';
import { compareCardWords } from '../shell/Sidebar';
import type { WorktreeAgent } from '../composer/types';
import { budgetCheck, elapsedWords, estimateFor, firstRows, nextRow, testWords, type CompareColumn, type CompareOptions } from './compare';

const t = translator([{
  'compare.menu': 'Compare run…', 'compare.menuHelp': 'Run the words with several agents',
  'composer.busySend.title': 'While running, send to', 'composer.busySend.queue': 'Queue', 'composer.busySend.worktree': 'Worktree · {agent}',
  'compare.test.none': 'No test', 'compare.test.waiting': 'After its report', 'compare.test.running': 'Testing…', 'compare.test.passed': 'Passed',
  'compare.test.failed': 'Failed (code {code})',
  'compare.card.title': 'Compare · {count}', 'compare.card.running': '{count} running', 'compare.card.reported': '{count} reported',
  'compare.card.failed': '{count} failed', 'compare.card.merged': '{count} merged', 'compare.card.discarded': '{count} discarded',
}]);

const options: CompareOptions = {
  available: true,
  max: 3,
  parentAgent: 'codex',
  agents: [
    { agent: 'codex', label: 'Codex', models: [{ id: 'gpt-x', label: 'GPT X' }] },
    { agent: 'claude', label: 'Claude', models: [{ id: 'opus', label: 'Opus' }] },
  ],
  budget: {
    usd: 10, spent: 8, fallback: 1,
    estimates: [{ agent: 'claude', model: 'opus', usd: 3 }, { agent: 'claude', model: null, usd: 1.5 }, { agent: 'codex', model: null, usd: 0.5 }],
  },
};

const column = (over: Partial<CompareColumn> = {}): CompareColumn => ({
  id: 'c', agent: 'claude', agentLabel: 'Claude', model: null, phase: 'reported', failure: null, files: 1, added: 2, removed: 0, elapsed: 65,
  running: false, costUsd: null, portBase: null, portCount: null, test: null, canOpen: true, canDiff: true, canMerge: true, canDiscard: true, branch: null,
  ...over,
});

describe('the compare picker', () => {
  it('starts with the pane’s own agent and another, and adds the least picked agent up to the room', () => {
    expect(firstRows(options)).toEqual([{ agent: 'codex', model: null }, { agent: 'claude', model: null }]);
    const one = { ...options, agents: [options.agents![1]!] };
    expect(firstRows(one), 'one agent twice: “2× Claude”').toEqual([{ agent: 'claude', model: null }, { agent: 'claude', model: null }]);
    expect(nextRow(options, [{ agent: 'codex', model: null }, { agent: 'codex', model: null }])).toEqual({ agent: 'claude', model: null });
    expect(nextRow(options, firstRows(options).concat([{ agent: 'codex', model: null }]))).toBeNull();
    expect(firstRows({ ...options, max: 1 })).toEqual([]);
  });

  it('estimates each row from the ledger and warns when the budget would be passed', () => {
    expect(estimateFor(options, { agent: 'claude', model: 'opus' })).toBe(3);
    expect(estimateFor(options, { agent: 'claude', model: 'haiku' })).toBe(1.5);
    expect(estimateFor(options, { agent: 'gemini', model: null })).toBe(1);
    expect(budgetCheck(options, [{ agent: 'codex', model: null }, { agent: 'claude', model: null }])).toEqual({ estimate: 2, spent: 8, budget: 10, over: false });
    expect(budgetCheck(options, [{ agent: 'codex', model: null }, { agent: 'claude', model: 'opus' }]).over).toBe(true);
    expect(budgetCheck({ ...options, budget: { ...options.budget!, usd: null } }, [{ agent: 'claude', model: 'opus' }]).over).toBe(false);
  });

  it('offers 비교 실행… as the last row of the busy send menu', () => {
    const agents: WorktreeAgent[] = [{ agent: 'claude', label: 'Claude', detail: '', enabled: true }, { agent: 'codex', label: 'Codex', detail: '', enabled: true }];
    const rows = busySendRows(agents, null, t, true);
    expect(rows.at(-1)).toMatchObject({ kind: 'action', text: 'Compare run…' });
    expect(isCompareChoice(agents, rows.length - 1)).toBe(true);
    expect(isCompareChoice(agents, rows.length - 2)).toBe(false);
    expect(busySendRows(agents, null, t).length, 'not offered: the menu as it was').toBe(rows.length - 2);
  });
});

describe('the compare view and card', () => {
  it('words the elapsed time and the test result', () => {
    expect(elapsedWords(65)).toBe('1:05');
    expect(elapsedWords(3725)).toBe('1:02:05');
    expect(elapsedWords(null)).toBe('—');
    expect(testWords(column(), false, t)).toBe('No test');
    expect(testWords(column(), true, t)).toBe('After its report');
    expect(testWords(column({ test: { state: 'failed', exitCode: 2, output: '' } }), true, t)).toBe('Failed (code 2)');
    expect(testWords(column({ test: { state: 'passed', exitCode: 0, output: '' } }), true, t)).toBe('Passed');
  });

  it('sums a group up on its sidebar card, leaving out the empty counts', () => {
    const words = compareCardWords({ id: 'g', title: 'Fix', total: 3, running: 1, reported: 1, failed: 1, merged: 0, discarded: 0 }, t);
    expect(words).toEqual({ title: 'Compare · 3', counts: '1 running · 1 reported · 1 failed' });
  });
});

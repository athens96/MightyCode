import { describe, expect, it } from 'vitest';
import { queueRowButtons, readRunNow, remoteRunNowTip, remoteRunNowWaits, rowAgent, runNowBranches, runNowTip, runNowWaits, type RunNow } from './queue';

const names: RunNow[] = ['worktree', 'agentMenu', 'here', 'steer', 'held', 'waitsForTurn', 'waitsForHeld', 'waitsForUpdate'];

describe('바로 실행 on a queued row', () => {
  it('reads the eight names the host sends and nothing else', () => {
    for (const name of names) expect(readRunNow(name)).toBe(name);
    for (const value of ['hidden', 'runNext', '', null, 1, undefined]) expect(readRunNow(value)).toBeNull();
  });

  it('gives each answer its tooltip (the Global Constraints table)', () => {
    expect(runNowTip('held', 3)).toEqual(['queue.runNowHeldHelp', { count: 3 }]);
    expect(runNowTip('worktree', 0, 'Codex')).toEqual(['queue.runNowAgentHelp', { agent: 'Codex' }]);
    const keys: Record<RunNow, string> = {
      worktree: 'queue.runNowAgentHelp', agentMenu: 'queue.runNowAgentHelp', here: 'queue.runNowHereHelp', steer: 'queue.runNowSteerHelp',
      held: 'queue.runNowHeldHelp', waitsForTurn: 'queue.runNowUnavailableHelp', waitsForHeld: 'queue.runNowAfterHeldHelp', waitsForUpdate: 'queue.runNowUpdateHelp',
    };
    for (const name of names) expect(runNowTip(name, 1)[0]).toBe(keys[name]);
  });

  it('draws only the three waiting answers disabled, and the branch on worktree rows', () => {
    expect(names.filter(runNowWaits)).toEqual(['waitsForTurn', 'waitsForHeld', 'waitsForUpdate']);
    expect(names.filter(runNowBranches)).toEqual(['worktree', 'agentMenu']);
  });

  it('runs a worktree row with the agent picked, else the pane\'s own, else the first that can run now', () => {
    const agents = [{ agent: 'claude', enabled: true }, { agent: 'codex', enabled: true }, { agent: 'gemini', enabled: false }];
    expect(rowAgent(agents, null, 'claude')?.agent).toBe('claude');
    expect(rowAgent(agents, 'codex', 'claude')?.agent).toBe('codex');
    expect(rowAgent(agents, 'gemini', 'claude')?.agent).toBe('claude');
    // A pane whose own agent cannot make a child in its mode starts on the first that can.
    expect(rowAgent([{ agent: 'codex', enabled: false }, { agent: 'claude', enabled: true }], null, 'codex')?.agent).toBe('claude');
    expect(rowAgent([{ agent: 'codex', enabled: false }], null, 'codex')).toBeNull();
    expect(rowAgent([], null, 'claude')).toBeNull();
  });
});

describe("a mirrored pane's queue (its host's, MC-49)", () => {
  it('draws this PC\'s own rows as before: 바로 실행 with the agent pick, and ×', () => {
    expect(queueRowButtons('worktree', undefined)).toEqual({ runNow: 'worktree', pick: true, remove: true });
    expect(queueRowButtons(null, null)).toEqual({ runNow: null, pick: true, remove: true });
  });

  it("draws a host's rows with × and 바로 실행 but no agent pick (the host pane's own agent runs it)", () => {
    for (const name of names) expect(queueRowButtons(name, { actions: true })).toEqual({ runNow: name, pick: false, remove: true });
    expect(queueRowButtons(null, { actions: true })).toEqual({ runNow: null, pick: false, remove: true });
  });

  it('draws an older host\'s rows without buttons, since it does not take the actions', () => {
    for (const name of [...names, null]) expect(queueRowButtons(name, { actions: false })).toEqual({ runNow: null, pick: false, remove: false });
  });

  it("lets a host's worktree row run, and makes a row that needs another agent's pick wait", () => {
    expect(remoteRunNowWaits('worktree')).toBe(false);
    expect(remoteRunNowWaits('here')).toBe(false);
    expect(remoteRunNowWaits('agentMenu')).toBe(true);
    for (const name of ['waitsForTurn', 'waitsForHeld', 'waitsForUpdate'] as RunNow[]) expect(remoteRunNowWaits(name)).toBe(true);
    expect(remoteRunNowTip('worktree', 0)).toEqual(['queue.runNowRemoteWorktreeHelp']);
    expect(remoteRunNowTip('agentMenu', 0)).toEqual(['queue.runNowUnavailableHelp']);
    expect(remoteRunNowTip('here', 0)).toEqual(runNowTip('here', 0));
    expect(remoteRunNowTip('waitsForUpdate', 2)).toEqual(runNowTip('waitsForUpdate', 2));
  });
});

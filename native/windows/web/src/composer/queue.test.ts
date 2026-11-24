import { describe, expect, it } from 'vitest';
import { editedWords, queueMoveTarget, queueRowButtons, readRunNow, remoteRunNowTip, remoteRunNowWaits, rowAgent, runNowBranches, runNowTip, runNowWaits, type RunNow } from './queue';

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

describe("a queued row's buttons, this PC's and a mirrored pane's host's (MC-49)", () => {
  it('draws this PC\'s own rows with 바로 실행 and its agent pick, ×, 고치기 and ↑/↓', () => {
    expect(queueRowButtons('worktree', undefined)).toEqual({ runNow: 'worktree', pick: true, remove: true, edit: true, move: true });
    expect(queueRowButtons(null, null)).toEqual({ runNow: null, pick: true, remove: true, edit: true, move: true });
    // An allowed card's follow-up keeps its words, but moves.
    expect(queueRowButtons('here', null, true)).toEqual({ runNow: 'here', pick: true, remove: true, edit: false, move: true });
  });

  it("draws a host's rows with what the host takes, never the agent pick (the host pane's own agent runs it)", () => {
    const all = { actions: true, edit: true, reorder: true };
    for (const name of names) expect(queueRowButtons(name, all)).toEqual({ runNow: name, pick: false, remove: true, edit: true, move: true });
    expect(queueRowButtons(null, all)).toEqual({ runNow: null, pick: false, remove: true, edit: true, move: true });
    // A host row whose words came cut is not edited (the rest would be lost).
    expect(queueRowButtons('here', all, true)).toEqual({ runNow: 'here', pick: false, remove: true, edit: false, move: true });
    // An earlier MC-49 host takes × and 바로 실행 only.
    expect(queueRowButtons('here', { actions: true })).toEqual({ runNow: 'here', pick: false, remove: true, edit: false, move: false });
  });

  it('draws an older host\'s rows without buttons, since it takes no queue action', () => {
    for (const name of [...names, null]) {
      expect(queueRowButtons(name, { actions: false, edit: false, reorder: false })).toEqual({ runNow: null, pick: false, remove: false, edit: false, move: false });
    }
  });

  it('moves a focused row one place with ↑/↓, never past either end', () => {
    expect(queueMoveTarget(1, 3, 'ArrowUp')).toBe(0);
    expect(queueMoveTarget(1, 3, 'ArrowDown')).toBe(2);
    expect(queueMoveTarget(0, 3, 'ArrowUp')).toBeNull();
    expect(queueMoveTarget(2, 3, 'ArrowDown')).toBeNull();
    expect(queueMoveTarget(0, 1, 'ArrowDown')).toBeNull();
    for (const key of ['Enter', 'ArrowLeft', 'k']) expect(queueMoveTarget(1, 3, key)).toBeNull();
  });

  it('saves 고치기\'s words as written, and refuses none on a row without files', () => {
    expect(editedWords('  new words \n', 0)).toBe('  new words \n');
    expect(editedWords('', 0)).toBeNull();
    expect(editedWords(' \n\t', 0)).toBeNull();
    expect(editedWords('', 2)).toBe('');
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
